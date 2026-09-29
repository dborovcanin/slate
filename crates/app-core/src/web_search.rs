use crate::{calc::CalcEngine, config::WebSearchConfig};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use url::Url;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// Caps the whole DuckDuckGo attempt. Each request has its own timeout and one
/// search can issue several of them, so without a shared deadline a slow
/// provider could hold the search overlay for the sum of them all.
const DUCKDUCKGO_BUDGET: Duration = Duration::from_secs(12);
const MAX_QUERY_CHARS: usize = 512;
const MAX_TITLE_CHARS: usize = 300;
const MAX_SNIPPET_CHARS: usize = 2_000;
const MIN_RESULTS: usize = 1;
const MAX_RESULTS: usize = 10;
const DUCKDUCKGO_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0";
/// Bounds the disambiguation follow-up chain so a cycle of topic pages cannot
/// turn one search into an unbounded series of requests.
const MAX_INSTANT_ANSWER_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub markdown_link: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchSource {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchAnswerCard {
    pub title: String,
    pub text: String,
    pub sources: Vec<WebSearchSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchResult {
    pub query: String,
    /// A provider-supplied direct answer, such as a unit conversion.
    pub answer: Option<String>,
    /// A descriptive result summary when no direct answer is available.
    pub summary: Option<String>,
    /// A presentation-ready answer with explicit supporting sources when available.
    pub answer_card: Option<WebSearchAnswerCard>,
    pub items: Vec<WebSearchItem>,
}

#[derive(Debug, Deserialize)]
struct DuckDuckGoTopic {
    #[serde(default, rename = "Text")]
    text: Option<String>,
    #[serde(default, rename = "FirstURL")]
    first_url: Option<String>,
    #[serde(default, rename = "Topics")]
    topics: Option<Vec<DuckDuckGoTopic>>,
}

#[derive(Debug, Deserialize)]
struct DuckDuckGoApiResponse {
    #[serde(default, rename = "Heading")]
    heading: Option<String>,
    #[serde(default, rename = "AbstractText")]
    abstract_text: Option<String>,
    #[serde(default, rename = "AbstractURL")]
    abstract_url: Option<String>,
    #[serde(default, rename = "AbstractSource")]
    abstract_source: Option<String>,
    #[serde(default, rename = "Answer")]
    answer: Option<String>,
    #[serde(default, rename = "RelatedTopics")]
    related_topics: Option<Vec<DuckDuckGoTopic>>,
    /// "A" for an article, "D" for a disambiguation page, "" when absent.
    #[serde(default, rename = "Type")]
    result_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleCustomSearchItem {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleCustomSearchResponse {
    #[serde(default)]
    items: Option<Vec<GoogleCustomSearchItem>>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BraveWebSection {
    #[serde(default)]
    results: Option<Vec<BraveWebResult>>,
}

#[derive(Debug, Deserialize)]
struct BraveInfoboxResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    long_desc: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BraveInfobox {
    #[serde(default)]
    results: Option<Vec<BraveInfoboxResult>>,
}

#[derive(Debug, Deserialize)]
struct BraveQuestion {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BraveFaq {
    #[serde(default)]
    results: Option<Vec<BraveQuestion>>,
}

#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<BraveWebSection>,
    #[serde(default)]
    infobox: Option<BraveInfobox>,
    #[serde(default)]
    faq: Option<BraveFaq>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchProvider {
    Brave,
    Google,
    DuckDuckGo,
}

impl SearchProvider {
    fn label(self) -> &'static str {
        match self {
            SearchProvider::Brave => "Brave",
            SearchProvider::Google => "Google",
            SearchProvider::DuckDuckGo => "DuckDuckGo",
        }
    }
}

impl WebSearchResult {
    fn empty(query: &str) -> Self {
        Self {
            query: query.to_string(),
            answer: None,
            summary: None,
            answer_card: None,
            items: Vec::new(),
        }
    }

    fn has_content(&self) -> bool {
        !self.items.is_empty()
            || self.answer.is_some()
            || self.summary.is_some()
            || self.answer_card.is_some()
    }
}

/// Keyed providers rank far better and carry richer answer text, so they run
/// first whenever credentials exist. The key-free DuckDuckGo path stays behind
/// them as a fallback so search still works with no configuration at all.
fn provider_chain(config: &WebSearchConfig) -> Result<Vec<SearchProvider>, String> {
    let has_api_key = !config.api_key.trim().is_empty();
    let has_engine_id = !config.search_engine_id.trim().is_empty();

    match config.provider.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => {
            // A search engine ID only exists for Google, so its presence is
            // what distinguishes the two key-based providers.
            let mut chain = Vec::new();
            if has_api_key && has_engine_id {
                chain.push(SearchProvider::Google);
            } else if has_api_key {
                chain.push(SearchProvider::Brave);
            }
            chain.push(SearchProvider::DuckDuckGo);
            Ok(chain)
        }
        "brave" => Ok(vec![SearchProvider::Brave, SearchProvider::DuckDuckGo]),
        "google" => Ok(vec![SearchProvider::Google, SearchProvider::DuckDuckGo]),
        "duckduckgo" => Ok(vec![SearchProvider::DuckDuckGo]),
        provider => Err(format!(
            "Unsupported web search provider '{provider}'; expected 'auto', 'brave', 'google', or 'duckduckgo'"
        )),
    }
}

pub fn search_web(query: &str, config: &WebSearchConfig) -> Result<WebSearchResult, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(WebSearchResult {
            query: String::new(),
            answer: None,
            summary: None,
            answer_card: None,
            items: Vec::new(),
        });
    }

    if trimmed.chars().count() > MAX_QUERY_CHARS {
        return Err(format!(
            "Web search query is too long (maximum {MAX_QUERY_CHARS} characters)"
        ));
    }

    // Calculator-style queries are deterministic and already belong to the
    // shared core. Resolve them locally instead of depending on a search
    // provider's optional instant-answer support.
    if let Some(answer) = local_calculation_answer(trimmed) {
        return Ok(WebSearchResult {
            query: trimmed.to_string(),
            answer_card: Some(WebSearchAnswerCard {
                title: "Calculation".to_string(),
                text: answer.clone(),
                sources: Vec::new(),
            }),
            answer: Some(answer),
            summary: None,
            items: Vec::new(),
        });
    }

    let chain = provider_chain(config)?;
    let mut errors = Vec::new();
    let mut searched_without_results = false;

    for provider in chain {
        let attempt = match provider {
            SearchProvider::Brave => search_brave(trimmed, config),
            SearchProvider::Google => search_google(trimmed, config),
            SearchProvider::DuckDuckGo => search_duckduckgo(trimmed, config.max_results),
        };

        match attempt {
            Ok(mut result) if result.has_content() => {
                apply_contextual_answer(&mut result, trimmed);
                return Ok(result);
            }
            // A provider that answered cleanly with nothing is a genuine empty
            // result, not a failure, so keep looking but do not report an error.
            Ok(_) => searched_without_results = true,
            Err(err) => errors.push(format!("{}: {err}", provider.label())),
        }
    }

    if searched_without_results {
        return Ok(WebSearchResult::empty(trimmed));
    }

    Err(errors.join("; "))
}

/// The question that was asked outranks whatever the provider chose to
/// summarise: when a result actually contains the requested fact, that becomes
/// the card. Shared across providers so a keyed one answers as well as the
/// key-free one, and only ever an upgrade, so a miss keeps the generic card.
fn apply_contextual_answer(result: &mut WebSearchResult, query: &str) {
    if result.answer.is_some() {
        return;
    }
    let Some(intent) = contextual_fact_intent(query) else {
        return;
    };
    if let Some(card) = build_contextual_answer_card(&intent, &result.items) {
        result.answer_card = Some(card);
    }
}

fn local_calculation_answer(query: &str) -> Option<String> {
    CalcEngine::new()
        .evaluate(query)
        .map(|answer| sanitize_result_text(&answer, MAX_SNIPPET_CHARS))
        .filter(|answer| !answer.is_empty())
}

pub fn search_duckduckgo(query: &str, max_results: usize) -> Result<WebSearchResult, String> {
    let max_results = max_results.clamp(MIN_RESULTS, MAX_RESULTS);
    let deadline = Instant::now() + DUCKDUCKGO_BUDGET;
    let fact_intent = contextual_fact_intent(query);
    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(DUCKDUCKGO_USER_AGENT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let mut answer: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut structured_source: Option<WebSearchSource> = None;
    let mut items: Vec<WebSearchItem> = Vec::new();
    let mut request_succeeded = false;
    let mut request_errors = Vec::new();

    // 1. First attempt DuckDuckGo Instant Answer API. If a natural-language
    // question has no answer, retry its subject (for example, "how tall is X"
    // becomes "X"), which is the form the topic API understands. A
    // disambiguation page adds its best-matching subject to the queue so the
    // concrete article is what ends up answering the question.
    let mut pending: VecDeque<String> = instant_answer_queries(query).into();
    let mut attempted: Vec<String> = Vec::new();

    while let Some(api_query) = pending.pop_front() {
        if attempted.len() >= MAX_INSTANT_ANSWER_ATTEMPTS || Instant::now() >= deadline {
            break;
        }
        if attempted
            .iter()
            .any(|previous| previous.eq_ignore_ascii_case(&api_query))
        {
            continue;
        }
        attempted.push(api_query.clone());

        let api_request = client.get("https://api.duckduckgo.com/").query(&[
            ("q", api_query.as_str()),
            ("format", "json"),
            ("no_html", "1"),
            ("skip_disambig", "1"),
            ("t", "slate-notes"),
        ]);

        match api_request.send() {
            Ok(resp) if resp.status().is_success() => match resp.json::<DuckDuckGoApiResponse>() {
                Ok(data) => {
                    request_succeeded = true;
                    let instant = collect_duckduckgo_api_response(data, &mut items, max_results);
                    if let Some(subject) = instant.disambiguation_subject {
                        pending.push_back(subject);
                        continue;
                    }
                    answer = instant.answer;
                    summary = instant.summary;
                    structured_source = instant.source;
                    if answer.is_some() || summary.is_some() || !items.is_empty() {
                        break;
                    }
                }
                Err(err) => request_errors.push(format!(
                    "DuckDuckGo instant-answer response was invalid: {err}"
                )),
            },
            Ok(resp) => request_errors.push(format!(
                "DuckDuckGo instant-answer request returned {}",
                resp.status()
            )),
            Err(err) => request_errors.push(format!(
                "DuckDuckGo instant-answer request failed: {}",
                err.without_url()
            )),
        }
    }

    // 2. Query DuckDuckGo HTML Lite when we need more items. Contextual fact
    // questions always use the original query so a generic topic abstract does
    // not crowd out snippets that actually contain the requested attribute.
    if (items.len() < max_results || fact_intent.is_some()) && Instant::now() < deadline {
        let html_url = "https://html.duckduckgo.com/html/";
        let params = [("q", query), ("b", "")];

        match client.post(html_url).form(&params).send() {
            Ok(resp) if resp.status().is_success() => match resp.text() {
                Ok(html) if is_duckduckgo_challenge(&html) => {
                    // The challenge page is served with a 2xx status, so
                    // without this check it parses to zero results and the
                    // scrape looks like a search that simply found nothing.
                    request_errors.push(
                        "DuckDuckGo served an anti-bot challenge instead of results; set an api_key in [web_search] to use a keyed provider"
                            .to_string(),
                    );
                }
                Ok(html) => {
                    request_succeeded = true;
                    let parsed_items = parse_duckduckgo_html(&html, max_results);
                    if fact_intent.is_some() {
                        items = merge_search_items(parsed_items, items, max_results);
                    } else {
                        items = merge_search_items(items, parsed_items, max_results);
                    }
                }
                Err(err) => request_errors
                    .push(format!("DuckDuckGo HTML response could not be read: {err}")),
            },
            Ok(resp) => request_errors.push(format!(
                "DuckDuckGo HTML request returned {}",
                resp.status()
            )),
            Err(err) => request_errors.push(format!(
                "DuckDuckGo HTML request failed: {}",
                err.without_url()
            )),
        }
    }

    // Report an error when every attempt failed, and also when the attempts
    // that did fail left us with nothing to show: silently returning an empty
    // result would hide an outage or a block behind a "no results" message.
    let produced_nothing = items.is_empty() && answer.is_none() && summary.is_none();
    if !request_succeeded || (produced_nothing && !request_errors.is_empty()) {
        return Err(if request_errors.is_empty() {
            "DuckDuckGo search failed".to_string()
        } else {
            request_errors.join("; ")
        });
    }

    items.truncate(max_results);
    if summary.is_none() {
        summary = first_meaningful_snippet(&items);
    }
    let answer_card = build_search_answer_card(
        answer.as_deref(),
        summary.as_deref(),
        structured_source.as_ref(),
        &items,
    );

    Ok(WebSearchResult {
        query: query.to_string(),
        answer,
        summary,
        answer_card,
        items,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ContextualFactIntent {
    Height { subject: String },
}

fn contextual_fact_intent(query: &str) -> Option<ContextualFactIntent> {
    const HEIGHT_PREFIXES: &[&str] = &[
        "how tall is ",
        "how high is ",
        "what is the height of ",
        "height of ",
    ];

    let lowercase = query.to_ascii_lowercase();
    for prefix in HEIGHT_PREFIXES {
        if lowercase.starts_with(prefix) {
            let subject = query[prefix.len()..]
                .trim()
                .trim_end_matches(['?', '!', '.'])
                .trim();
            if !subject.is_empty() {
                return Some(ContextualFactIntent::Height {
                    subject: subject.to_string(),
                });
            }
        }
    }
    None
}

fn instant_answer_queries(query: &str) -> Vec<String> {
    const QUESTION_PREFIXES: &[&str] = &[
        "how tall is ",
        "how high is ",
        "how old is ",
        "how long is ",
        "who is ",
        "what is ",
        "where is ",
        "tell me about ",
    ];

    let mut queries = vec![query.to_string()];
    let lowercase = query.to_ascii_lowercase();
    for prefix in QUESTION_PREFIXES {
        if lowercase.starts_with(prefix) {
            let subject = query[prefix.len()..]
                .trim()
                .trim_end_matches(['?', '!', '.'])
                .trim();
            if !subject.is_empty() && !subject.eq_ignore_ascii_case(query) {
                queries.push(subject.to_string());
            }
            break;
        }
    }
    queries
}

pub fn search_brave(query: &str, config: &WebSearchConfig) -> Result<WebSearchResult, String> {
    let api_key = config.api_key.trim();
    if api_key.is_empty() {
        return Err("Brave web search requires 'api_key' in [web_search] config".to_string());
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let max_results = config.max_results.clamp(MIN_RESULTS, MAX_RESULTS);
    let count = max_results.to_string();

    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("Accept", "application/json")
        .header("X-Subscription-Token", api_key)
        .query(&[
            ("q", query),
            ("count", count.as_str()),
            ("result_filter", "web,infobox,faq"),
        ])
        .send()
        .map_err(|e| format!("Brave search request failed: {}", e.without_url()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Brave search returned error status {status}"));
    }

    let data = resp
        .json::<BraveSearchResponse>()
        .map_err(|e| format!("Failed to parse Brave search response: {}", e.without_url()))?;

    let mut items = Vec::new();
    for raw in data.web.and_then(|web| web.results).unwrap_or_default() {
        if items.len() >= max_results {
            break;
        }
        // Brave marks query terms with <strong> inside descriptions.
        let title = strip_html_tags(raw.title.as_deref().unwrap_or_default());
        let url = raw.url.unwrap_or_default();
        let snippet = strip_html_tags(raw.description.as_deref().unwrap_or_default());
        if let Some(item) = web_search_item(&title, &url, &snippet) {
            items.push(item);
        }
    }

    // Brave's own question answering is the closest thing it offers to a
    // direct answer, so it outranks the infobox, which in turn describes the
    // subject more fully than any single result snippet.
    let faq_answer = data
        .faq
        .and_then(|faq| faq.results)
        .unwrap_or_default()
        .into_iter()
        .find_map(|entry| {
            brave_passage(
                entry.answer.as_deref(),
                entry.title.as_deref(),
                entry.url.as_deref(),
            )
        });

    let infobox_answer = data
        .infobox
        .and_then(|infobox| infobox.results)
        .unwrap_or_default()
        .into_iter()
        .find_map(|entry| {
            brave_passage(
                entry.long_desc.as_deref().or(entry.description.as_deref()),
                entry.title.as_deref(),
                entry.url.as_deref(),
            )
        });

    let (summary, structured_source) = match faq_answer.or(infobox_answer) {
        Some((text, source)) => (Some(text), source),
        None => (first_meaningful_snippet(&items), None),
    };

    let answer_card =
        build_search_answer_card(None, summary.as_deref(), structured_source.as_ref(), &items);

    Ok(WebSearchResult {
        query: query.to_string(),
        answer: None,
        summary,
        answer_card,
        items,
    })
}

/// Turns one of Brave's descriptive blocks into answer text plus the source it
/// came from, discarding blocks that carry no usable prose.
fn brave_passage(
    text: Option<&str>,
    title: Option<&str>,
    url: Option<&str>,
) -> Option<(String, Option<WebSearchSource>)> {
    let text = sanitize_result_text(
        &strip_html_tags(text.unwrap_or_default()),
        MAX_SNIPPET_CHARS,
    );
    if text.is_empty() {
        return None;
    }
    let source = title
        .zip(url)
        .and_then(|(title, url)| web_search_source(&strip_html_tags(title), url));
    Some((text, source))
}

pub fn search_google(query: &str, config: &WebSearchConfig) -> Result<WebSearchResult, String> {
    let api_key = config.api_key.trim();
    let search_engine_id = config.search_engine_id.trim();

    if api_key.is_empty() || search_engine_id.is_empty() {
        return Err(
            "Legacy Google web search requires 'api_key' and 'search_engine_id' in [web_search] config"
                .to_string(),
        );
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let max_results = config.max_results.clamp(MIN_RESULTS, MAX_RESULTS);
    let max_results_param = max_results.to_string();

    let resp = client
        .get("https://www.googleapis.com/customsearch/v1")
        .query(&[
            ("key", api_key),
            ("cx", search_engine_id),
            ("q", query),
            ("num", max_results_param.as_str()),
        ])
        .send()
        .map_err(|e| format!("Google search request failed: {}", e.without_url()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Google search returned error status {status}"));
    }

    let data = resp.json::<GoogleCustomSearchResponse>().map_err(|e| {
        format!(
            "Failed to parse Google search response: {}",
            e.without_url()
        )
    })?;

    let mut items = Vec::new();
    let mut summary = None;

    if let Some(raw_items) = data.items {
        for item in raw_items {
            let title = item.title.unwrap_or_default();
            let link = item.link.unwrap_or_default();
            let snippet = item.snippet.unwrap_or_default();
            let Some(item) = web_search_item(&title, &link, &snippet) else {
                continue;
            };

            if summary.is_none() && !item.snippet.is_empty() {
                summary = Some(item.snippet.clone());
            }

            items.push(item);

            if items.len() >= max_results {
                break;
            }
        }
    }

    let answer_card = build_search_answer_card(None, summary.as_deref(), None, &items);

    Ok(WebSearchResult {
        query: query.to_string(),
        answer: None,
        summary,
        answer_card,
        items,
    })
}

#[derive(Debug, Default)]
struct DuckDuckGoInstantAnswer {
    answer: Option<String>,
    summary: Option<String>,
    source: Option<WebSearchSource>,
    /// Set when DuckDuckGo answered with a disambiguation page. Its related
    /// topics list other subjects that merely share a name, so they are
    /// navigation entries rather than results for this query.
    disambiguation_subject: Option<String>,
}

fn collect_duckduckgo_api_response(
    data: DuckDuckGoApiResponse,
    items: &mut Vec<WebSearchItem>,
    max_results: usize,
) -> DuckDuckGoInstantAnswer {
    // A disambiguation page describes the name, not the subject the question
    // asked about, so presenting its abstract or topic list would answer "who
    // shares this name" instead of the query. Resolve it to a concrete subject
    // and let the caller ask again.
    if data.result_type.as_deref() == Some("D") {
        return DuckDuckGoInstantAnswer {
            disambiguation_subject: data
                .related_topics
                .as_deref()
                .and_then(best_disambiguation_subject),
            ..DuckDuckGoInstantAnswer::default()
        };
    }

    let answer = sanitized_optional_text(data.answer);
    let summary = sanitized_optional_text(data.abstract_text);
    let mut source = None;

    if let (Some(url), Some(title)) = (
        data.abstract_url.filter(|url| !url.trim().is_empty()),
        data.heading
            .or(data.abstract_source)
            .filter(|title| !title.trim().is_empty()),
    ) {
        source = web_search_source(&title, &url);
        let snippet = summary.as_deref().or(answer.as_deref()).unwrap_or_default();
        if let Some(item) = web_search_item(&title, &url, snippet) {
            items.push(item);
        }
    }

    if let Some(topics) = data.related_topics {
        collect_ddg_topics(&topics, items, max_results);
    }

    DuckDuckGoInstantAnswer {
        answer,
        summary,
        source,
        disambiguation_subject: None,
    }
}

/// DuckDuckGo orders disambiguation topics by relevance, so the first entry
/// that names a subject is the one the query most likely meant.
fn best_disambiguation_subject(topics: &[DuckDuckGoTopic]) -> Option<String> {
    for topic in topics {
        if let Some(subtopics) = &topic.topics {
            if let Some(subject) = best_disambiguation_subject(subtopics) {
                return Some(subject);
            }
            continue;
        }

        let url = topic.first_url.as_deref().unwrap_or_default().trim();
        if url.is_empty() {
            continue;
        }
        let subject = duckduckgo_topic_title(url).or_else(|| {
            let text = topic.text.as_deref()?.trim();
            (!text.is_empty()).then(|| split_topic_text(text, url).0)
        })?;
        let subject = subject.trim();
        if !subject.is_empty() {
            return Some(subject.to_string());
        }
    }
    None
}

/// DuckDuckGo answers a blocked scrape with its "bots use DuckDuckGo too"
/// challenge page under a 2xx status, so the body is what identifies it.
fn is_duckduckgo_challenge(html: &str) -> bool {
    html.contains("anomaly-modal") || html.contains("anomaly.js") || html.contains("challenge-form")
}

fn sanitized_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|text| sanitize_result_text(&text, MAX_SNIPPET_CHARS))
        .filter(|text| !text.is_empty())
}

fn collect_ddg_topics(
    topics: &[DuckDuckGoTopic],
    items: &mut Vec<WebSearchItem>,
    max_results: usize,
) {
    for topic in topics {
        if items.len() >= max_results {
            break;
        }
        if let Some(subtopics) = &topic.topics {
            collect_ddg_topics(subtopics, items, max_results);
            continue;
        }

        if let (Some(url), Some(text)) = (&topic.first_url, &topic.text) {
            let u = url.trim();
            let t = text.trim();
            if u.is_empty() || t.is_empty() || is_duckduckgo_category(u) {
                continue;
            }
            let (title, snippet) = split_topic_text(t, u);
            if let Some(item) = web_search_item(&title, u, &snippet) {
                items.push(item);
            }
        }
    }
}

fn split_topic_text(text: &str, url: &str) -> (String, String) {
    if let Some((title, rest)) = text.split_once(" - ") {
        (title.trim().to_string(), rest.trim().to_string())
    } else if let Some((title, rest)) = text.split_once(" — ") {
        (title.trim().to_string(), rest.trim().to_string())
    } else if let Some(title) = duckduckgo_topic_title(url) {
        let Some(prefix) = text.get(..title.len()) else {
            return (text.to_string(), String::new());
        };
        let remainder = text
            .get(title.len()..)
            .filter(|rest| {
                rest.chars()
                    .next()
                    .is_some_and(|ch| ch.is_whitespace() || matches!(ch, ':' | '-' | '—'))
            })
            .map(|rest| {
                rest.trim_start_matches(|ch: char| {
                    ch.is_whitespace() || matches!(ch, ':' | '-' | '—')
                })
            })
            .unwrap_or_default();

        if prefix.eq_ignore_ascii_case(&title) && !remainder.is_empty() {
            (prefix.to_string(), remainder.to_string())
        } else {
            (text.to_string(), String::new())
        }
    } else {
        (text.to_string(), String::new())
    }
}

/// DuckDuckGo hangs category listings such as "/c/Barry_University_alumni" off
/// every article. They are navigation for the subject, never results for the
/// query, so they only crowd out real links.
fn is_duckduckgo_category(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    parsed.host_str().is_some_and(|host| {
        host.trim_start_matches("www.")
            .eq_ignore_ascii_case("duckduckgo.com")
    }) && parsed.path().starts_with("/c/")
}

fn duckduckgo_topic_title(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?.trim_start_matches("www.");
    if !host.eq_ignore_ascii_case("duckduckgo.com") {
        return None;
    }
    let slug = parsed.path_segments()?.next_back()?.trim();
    if slug.is_empty() {
        return None;
    }
    let title = slug.replace('_', " ");
    (!title.is_empty()).then_some(title)
}

fn first_meaningful_snippet(items: &[WebSearchItem]) -> Option<String> {
    items
        .iter()
        .map(|item| item.snippet.trim())
        .find(|snippet| !snippet.is_empty())
        .map(ToString::to_string)
}

fn merge_search_items(
    primary: Vec<WebSearchItem>,
    secondary: Vec<WebSearchItem>,
    max_results: usize,
) -> Vec<WebSearchItem> {
    let mut merged = Vec::with_capacity(max_results);
    for item in primary.into_iter().chain(secondary) {
        if merged.len() >= max_results {
            break;
        }
        if !merged
            .iter()
            .any(|existing: &WebSearchItem| existing.url == item.url)
        {
            merged.push(item);
        }
    }
    merged
}

fn build_contextual_answer_card(
    intent: &ContextualFactIntent,
    items: &[WebSearchItem],
) -> Option<WebSearchAnswerCard> {
    match intent {
        ContextualFactIntent::Height { subject } => items.iter().find_map(|item| {
            let text = extract_height_context(subject, &item.snippet)?;
            let source = web_search_source(&item.title, &item.url)?;
            Some(WebSearchAnswerCard {
                title: "Source-backed answer".to_string(),
                text,
                sources: vec![source],
            })
        }),
    }
}

const HEIGHT_MARKERS: &[&str] = &["height", "tall", "stands at", "standing at", "stature"];

fn extract_height_context(subject: &str, text: &str) -> Option<String> {
    // Heights appear both spaced ("7 ft 1 in") and hyphenated
    // ("7-foot-1-inch"); the hyphenated form usually appears mid-sentence with
    // no marker word ("he is a 7-foot-1-inch center"), so separators are
    // optional and a compound feet-and-inches reading stands on its own.
    let measurement_re = Regex::new(
        r"(?i)\b\d{1,4}(?:[.,]\d+)?[\s\-]*(?:m|metres?|meters?|cm|centimetres?|centimeters?|ft|feet|foot)\b(?:[\s\-]*\d{1,2}[\s\-]*(?:in|inch|inches)\b)?(?:\s*\([^)]{1,48}\))?",
    )
    .ok()?;
    let compound_re = Regex::new(r"(?i)(?:in|inch|inches)\s*\)?$").ok()?;
    let subject_lowercase = subject.to_ascii_lowercase();
    let mut fallback = None;

    for measurement in measurement_re.find_iter(text) {
        let start = context_start(text, measurement.start());
        let end = context_end(text, measurement.end());
        let fragment = sanitize_result_text(&text[start..end], MAX_SNIPPET_CHARS);
        let lowercase = fragment.to_ascii_lowercase();

        // A fragment that names the measurement as a height is unambiguous, so
        // return its wording as-is.
        if HEIGHT_MARKERS
            .iter()
            .any(|marker| lowercase.contains(marker))
        {
            if lowercase.starts_with("height") || lowercase.starts_with("stature") {
                return Some(format!("{subject} — {fragment}"));
            }
            return Some(fragment);
        }

        // Without a marker only a feet-and-inches reading, or one in a
        // fragment that names the subject, is confident enough to report, and
        // then only the measurement itself is worth quoting.
        let reading = sanitize_result_text(measurement.as_str(), MAX_TITLE_CHARS);
        if fallback.is_none()
            && !reading.is_empty()
            && (compound_re.is_match(&reading) || lowercase.contains(&subject_lowercase))
        {
            fallback = Some(format!("{subject} — {reading}"));
        }
    }

    fallback
}

fn context_start(text: &str, before: usize) -> usize {
    let prefix = &text[..before];
    let char_boundary = [';', '|', '\n']
        .iter()
        .filter_map(|separator| prefix.rfind(*separator).map(|index| index + 1))
        .max()
        .unwrap_or(0);
    let sentence_boundary = prefix.rfind(". ").map(|index| index + 2).unwrap_or(0);
    char_boundary.max(sentence_boundary)
}

fn context_end(text: &str, after: usize) -> usize {
    let suffix = &text[after..];
    let char_boundary = [';', '|', '\n']
        .iter()
        .filter_map(|separator| suffix.find(*separator))
        .min();
    let sentence_boundary = suffix.find(". ").map(|index| index + 1);
    char_boundary
        .into_iter()
        .chain(sentence_boundary)
        .min()
        .map(|offset| after + offset)
        .unwrap_or(text.len())
}

fn web_search_source(title: &str, url: &str) -> Option<WebSearchSource> {
    let title = sanitize_result_text(title, MAX_TITLE_CHARS);
    let url = normalize_http_url(url)?;
    if title.is_empty() {
        return None;
    }
    Some(WebSearchSource { title, url })
}

fn build_search_answer_card(
    answer: Option<&str>,
    summary: Option<&str>,
    structured_source: Option<&WebSearchSource>,
    items: &[WebSearchItem],
) -> Option<WebSearchAnswerCard> {
    if let Some(text) = answer.filter(|text| !text.trim().is_empty()) {
        let sources = structured_source.cloned().into_iter().collect::<Vec<_>>();
        return Some(WebSearchAnswerCard {
            title: if sources.is_empty() {
                "Answer".to_string()
            } else {
                "Source-backed answer".to_string()
            },
            text: text.to_string(),
            sources,
        });
    }

    let text = summary.filter(|text| !text.trim().is_empty())?;
    if let Some(source) = structured_source {
        return Some(WebSearchAnswerCard {
            title: "Source-backed answer".to_string(),
            text: text.to_string(),
            sources: vec![source.clone()],
        });
    }

    let source = items
        .iter()
        .find(|item| item.snippet.trim() == text.trim())
        .and_then(|item| web_search_source(&item.title, &item.url));
    Some(WebSearchAnswerCard {
        title: "Best result".to_string(),
        text: text.to_string(),
        sources: source.into_iter().collect(),
    })
}

pub fn parse_duckduckgo_html(html: &str, max_results: usize) -> Vec<WebSearchItem> {
    let mut results = Vec::new();
    let result_blocks: Vec<&str> = html.split("class=\"result ").collect();

    let link_re =
        Regex::new(r#"<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#).ok();
    let snippet_re = Regex::new(
        r#"(?s)<(?:a|div|span)[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</(?:a|div|span)>"#,
    )
    .ok();

    if let Some(link_re) = link_re {
        for block in result_blocks.into_iter().skip(1) {
            if results.len() >= max_results {
                break;
            }

            if let Some(caps) = link_re.captures(block) {
                let raw_href = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
                let raw_title = caps.get(2).map(|m| m.as_str()).unwrap_or_default();

                let url = decode_ddg_redirect_url(raw_href);
                let title = strip_html_tags(raw_title);

                let snippet = snippet_re
                    .as_ref()
                    .and_then(|snippet_re| snippet_re.captures(block))
                    .and_then(|c| c.get(1))
                    .map(|m| strip_html_tags(m.as_str()))
                    .unwrap_or_default();

                if let Some(item) = web_search_item(&title, &url, &snippet) {
                    results.push(item);
                }
            }
        }
    }

    results
}

pub fn decode_ddg_redirect_url(href: &str) -> String {
    let unescaped = decode_html_entities(href);
    let full_url = if unescaped.starts_with("//") {
        format!("https:{unescaped}")
    } else if unescaped.starts_with('/') {
        format!("https://duckduckgo.com{unescaped}")
    } else {
        unescaped.clone()
    };

    if let Ok(parsed) = Url::parse(&full_url) {
        for (k, v) in parsed.query_pairs() {
            if k == "uddg" {
                return v.into_owned();
            }
        }
        return full_url;
    }
    unescaped
}

pub fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(ch);
        }
    }
    decode_html_entities(out.trim())
}

pub fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

fn web_search_item(title: &str, url: &str, snippet: &str) -> Option<WebSearchItem> {
    let title = sanitize_result_text(title, MAX_TITLE_CHARS);
    let url = normalize_http_url(url)?;
    if title.is_empty() {
        return None;
    }
    let snippet = sanitize_result_text(snippet, MAX_SNIPPET_CHARS);
    let markdown_label = title.replace('[', "(").replace(']', ")");
    let markdown_url = url.replace('(', "%28").replace(')', "%29");
    let markdown_link = format!("[{markdown_label}]({markdown_url})");
    Some(WebSearchItem {
        title,
        url,
        snippet,
        markdown_link,
    })
}

fn normalize_http_url(raw: &str) -> Option<String> {
    let parsed = Url::parse(raw.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    Some(parsed.to_string())
}

fn sanitize_result_text(raw: &str, max_chars: usize) -> String {
    let mut sanitized = String::with_capacity(raw.len().min(max_chars));
    let mut pending_space = false;
    let mut chars = 0usize;

    for ch in raw.trim().chars() {
        if ch.is_control() || ch.is_whitespace() {
            pending_space = !sanitized.is_empty();
            continue;
        }
        if pending_space {
            if chars >= max_chars {
                break;
            }
            sanitized.push(' ');
            chars += 1;
            pending_space = false;
        }
        if chars >= max_chars {
            break;
        }
        sanitized.push(ch);
        chars += 1;
    }

    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_entities_and_tag_stripping() {
        let raw = "<b>Rust &amp; C++</b> &quot;Guide&#39;s&quot;";
        assert_eq!(strip_html_tags(raw), "Rust & C++ \"Guide's\"");
    }

    #[test]
    fn ddg_redirect_url_decoding() {
        let redirect = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&rut=1";
        assert_eq!(
            decode_ddg_redirect_url(redirect),
            "https://www.rust-lang.org/"
        );
    }

    #[test]
    fn parse_ddg_html_extracts_items() {
        let html = r#"
        <div class="result results_links">
          <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F">Rust Programming Language</a>
          <a class="result__snippet">A language empowering everyone to build reliable and efficient software.</a>
        </div>
        <div class="result results_links">
          <a class="result__a" href="https://doc.rust-lang.org/book/">The Rust Book</a>
          <div class="result__snippet">
            Official <b>documentation</b> and book for Rust.
          </div>
        </div>
        "#;
        let items = parse_duckduckgo_html(html, 5);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Rust Programming Language");
        assert_eq!(items[0].url, "https://www.rust-lang.org/");
        assert_eq!(
            items[0].snippet,
            "A language empowering everyone to build reliable and efficient software."
        );
        assert_eq!(items[1].title, "The Rust Book");
        assert_eq!(items[1].url, "https://doc.rust-lang.org/book/");
        assert_eq!(
            items[1].snippet,
            "Official documentation and book for Rust."
        );
    }

    #[test]
    fn ddg_instant_answer_is_distinct_from_result_summary() {
        let data: DuckDuckGoApiResponse = serde_json::from_str(
            r#"{
                "Answer": "35 cm = 13.7795 inches",
                "AbstractText": "A centimetre is a unit of length.",
                "AbstractURL": "https://example.com/centimetre",
                "Heading": "Centimetre",
                "RelatedTopics": []
            }"#,
        )
        .expect("valid fixture");
        let mut items = Vec::new();

        let instant = collect_duckduckgo_api_response(data, &mut items, 5);
        let (answer, summary, source) = (instant.answer, instant.summary, instant.source);

        assert_eq!(answer.as_deref(), Some("35 cm = 13.7795 inches"));
        assert_eq!(
            summary.as_deref(),
            Some("A centimetre is a unit of length.")
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].snippet, "A centimetre is a unit of length.");
        assert_eq!(
            source,
            Some(WebSearchSource {
                title: "Centimetre".to_string(),
                url: "https://example.com/centimetre".to_string(),
            })
        );
        assert_eq!(
            build_search_answer_card(
                answer.as_deref(),
                summary.as_deref(),
                source.as_ref(),
                &items,
            ),
            Some(WebSearchAnswerCard {
                title: "Source-backed answer".to_string(),
                text: "35 cm = 13.7795 inches".to_string(),
                sources: vec![WebSearchSource {
                    title: "Centimetre".to_string(),
                    url: "https://example.com/centimetre".to_string(),
                }],
            })
        );
    }

    #[test]
    fn calculator_query_returns_a_local_direct_answer() {
        let result = search_web("35cm to inches", &WebSearchConfig::default())
            .expect("conversion should not require a network request");
        let answer = result.answer.expect("direct conversion answer");

        assert!(answer.contains("13.779"), "unexpected answer: {answer}");
        assert!(answer.to_ascii_lowercase().contains("inch"));
        assert!(result.summary.is_none());
        assert!(result.items.is_empty());
        assert_eq!(
            result.answer_card,
            Some(WebSearchAnswerCard {
                title: "Calculation".to_string(),
                text: answer,
                sources: Vec::new(),
            })
        );
    }

    #[test]
    fn natural_language_question_retries_the_topic_subject() {
        assert_eq!(
            instant_answer_queries("How tall is Burj Khalifa?"),
            vec![
                "How tall is Burj Khalifa?".to_string(),
                "Burj Khalifa".to_string()
            ]
        );
        assert_eq!(
            instant_answer_queries("Burj Khalifa"),
            vec!["Burj Khalifa".to_string()]
        );
    }

    #[test]
    fn topic_text_splitting() {
        let (title, snippet) = split_topic_text(
            "Rust (programming language) - A systems language",
            "https://duckduckgo.com/Rust_(programming_language)",
        );
        assert_eq!(title, "Rust (programming language)");
        assert_eq!(snippet, "A systems language");

        let (title, snippet) = split_topic_text(
            "Novak Djokovic A Serbian professional tennis player.",
            "https://duckduckgo.com/Novak_Djokovic",
        );
        assert_eq!(title, "Novak Djokovic");
        assert_eq!(snippet, "A Serbian professional tennis player.");
    }

    #[test]
    fn summary_falls_back_to_first_non_empty_result_text() {
        let items = vec![
            web_search_item("Novak", "https://example.com/novak", "")
                .expect("valid empty-snippet result"),
            web_search_item(
                "Novak Djokovic",
                "https://example.com/djokovic",
                "A Serbian professional tennis player.",
            )
            .expect("valid result with text"),
        ];

        assert_eq!(
            first_meaningful_snippet(&items).as_deref(),
            Some("A Serbian professional tennis player.")
        );

        assert_eq!(
            build_search_answer_card(
                None,
                first_meaningful_snippet(&items).as_deref(),
                None,
                &items,
            ),
            Some(WebSearchAnswerCard {
                title: "Best result".to_string(),
                text: "A Serbian professional tennis player.".to_string(),
                sources: vec![WebSearchSource {
                    title: "Novak Djokovic".to_string(),
                    url: "https://example.com/djokovic".to_string(),
                }],
            })
        );
    }

    #[test]
    fn result_items_reject_unsafe_urls_and_build_safe_markdown() {
        assert!(web_search_item("bad", "javascript:alert(1)", "").is_none());
        assert!(web_search_item("bad", "file:///tmp/note", "").is_none());

        let item = web_search_item(
            "Rust [guide]\nnext",
            "https://example.com/a_(b)?q=1",
            "line one\u{1b}[31m\nline two",
        )
        .expect("safe web result");
        assert_eq!(item.title, "Rust [guide] next");
        assert_eq!(item.snippet, "line one [31m line two");
        assert_eq!(
            item.markdown_link,
            "[Rust (guide) next](https://example.com/a_%28b%29?q=1)"
        );
    }

    #[test]
    fn search_rejects_unsupported_provider_and_oversized_query() {
        let mut config = WebSearchConfig {
            provider: "unknown".to_string(),
            ..WebSearchConfig::default()
        };
        assert!(search_web("rust", &config)
            .expect_err("unknown provider rejected")
            .contains("Unsupported web search provider"));

        config.provider = "duckduckgo".to_string();
        let query = "x".repeat(MAX_QUERY_CHARS + 1);
        assert!(search_web(&query, &config)
            .expect_err("oversized query rejected")
            .contains("too long"));
    }

    #[test]
    fn disambiguation_page_resolves_to_a_concrete_subject() {
        // "shaq" resolves to a page about the name, whose related topics are
        // other people who share it. Presenting those as results answered
        // "who is called Shaq" instead of the question that was asked.
        let data: DuckDuckGoApiResponse = serde_json::from_str(
            r#"{
                "Type": "D",
                "Heading": "Shaq",
                "AbstractURL": "https://en.wikipedia.org/wiki/Shaq_(disambiguation)",
                "RelatedTopics": [
                    {
                        "Text": "Shaquille O'Neal An American former professional basketball player.",
                        "FirstURL": "https://duckduckgo.com/Shaquille_O'Neal"
                    },
                    {
                        "Text": "Shaq Barrett An American former professional football linebacker.",
                        "FirstURL": "https://duckduckgo.com/Shaquil_Barrett"
                    }
                ]
            }"#,
        )
        .expect("valid fixture");
        let mut items = Vec::new();

        let instant = collect_duckduckgo_api_response(data, &mut items, 10);

        assert_eq!(
            instant.disambiguation_subject.as_deref(),
            Some("Shaquille O'Neal")
        );
        assert!(items.is_empty(), "unexpected results: {items:?}");
        assert!(instant.summary.is_none());
        assert!(instant.answer.is_none());
    }

    #[test]
    fn article_pages_still_populate_results() {
        let data: DuckDuckGoApiResponse = serde_json::from_str(
            r#"{
                "Type": "A",
                "Heading": "Shaquille O'Neal",
                "AbstractText": "An American former professional basketball player.",
                "AbstractURL": "https://en.wikipedia.org/wiki/Shaquille_O'Neal",
                "RelatedTopics": []
            }"#,
        )
        .expect("valid fixture");
        let mut items = Vec::new();

        let instant = collect_duckduckgo_api_response(data, &mut items, 10);

        assert!(instant.disambiguation_subject.is_none());
        assert_eq!(items.len(), 1);
        assert_eq!(
            instant.summary.as_deref(),
            Some("An American former professional basketball player.")
        );
    }

    #[test]
    fn anti_bot_challenge_is_not_mistaken_for_an_empty_result_page() {
        let challenge = r#"<html><body>
            <form id="challenge-form" action="//duckduckgo.com/anomaly.js?sv=html" method="POST">
            <div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div>
            </form></body></html>"#;
        assert!(is_duckduckgo_challenge(challenge));
        assert!(parse_duckduckgo_html(challenge, 10).is_empty());

        let results = r#"<div class="result results_links">
            <a class="result__a" href="https://www.rust-lang.org/">Rust</a>
            <div class="result__snippet">A language empowering everyone.</div>
            </div>"#;
        assert!(!is_duckduckgo_challenge(results));
    }

    #[test]
    fn duckduckgo_category_topics_are_not_results() {
        assert!(is_duckduckgo_category(
            "https://duckduckgo.com/c/Barry_University_alumni"
        ));
        assert!(!is_duckduckgo_category(
            "https://duckduckgo.com/Shaquille_O'Neal"
        ));
        assert!(!is_duckduckgo_category(
            "https://en.wikipedia.org/wiki/Shaquille_O'Neal"
        ));

        let data: DuckDuckGoApiResponse = serde_json::from_str(
            r#"{
                "Type": "A",
                "Heading": "Shaquille O'Neal",
                "AbstractText": "An American former professional basketball player.",
                "AbstractURL": "https://en.wikipedia.org/wiki/Shaquille_O'Neal",
                "RelatedTopics": [
                    {
                        "Text": "Barry University alumni",
                        "FirstURL": "https://duckduckgo.com/c/Barry_University_alumni"
                    }
                ]
            }"#,
        )
        .expect("valid fixture");
        let mut items = Vec::new();

        collect_duckduckgo_api_response(data, &mut items, 10);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].url,
            "https://en.wikipedia.org/wiki/Shaquille_O'Neal"
        );
    }

    #[test]
    fn height_answer_reads_a_hyphenated_measurement_without_a_marker_word() {
        // Wikipedia-sourced abstracts write heights inline and never say
        // "tall", so requiring a marker word missed the answer entirely.
        let abstract_text = "Shaquille Rashaun O'Neal, commonly known as Shaq, is an American \
former professional basketball player. Nicknamed, among others, \"Diesel\", he is a \
7-foot-1-inch and 325-pound center who played for six teams over his 19-year career.";

        assert_eq!(
            extract_height_context("Shaq", abstract_text).as_deref(),
            Some("Shaq — 7-foot-1-inch")
        );

        // A fragment that names the measurement as a height keeps its wording.
        assert_eq!(
            extract_height_context(
                "Burj Khalifa",
                "The Burj Khalifa has a total height of 829.8 m and remains the tallest structure."
            )
            .as_deref(),
            Some(
                "The Burj Khalifa has a total height of 829.8 m and remains the tallest structure."
            )
        );

        // A weight is not a height, so nothing should be reported.
        assert_eq!(
            extract_height_context("Shaq", "He weighed 325 pounds during his playing career."),
            None
        );
    }

    #[test]
    fn contextual_height_card_cites_the_result_it_came_from() {
        let items = vec![web_search_item(
            "Shaquille O'Neal",
            "https://en.wikipedia.org/wiki/Shaquille_O%27Neal",
            "Nicknamed Diesel, he is a 7-foot-1-inch center.",
        )
        .expect("valid result")];

        let card = build_contextual_answer_card(
            &ContextualFactIntent::Height {
                subject: "shaq".to_string(),
            },
            &items,
        )
        .expect("height card");

        assert_eq!(card.text, "shaq — 7-foot-1-inch");
        assert_eq!(card.sources.len(), 1);
        assert_eq!(card.sources[0].title, "Shaquille O'Neal");
    }

    #[test]
    fn provider_chain_prefers_keys_and_always_keeps_a_key_free_fallback() {
        let keyless = WebSearchConfig {
            provider: "auto".to_string(),
            ..WebSearchConfig::default()
        };
        assert_eq!(
            provider_chain(&keyless).expect("valid chain"),
            vec![SearchProvider::DuckDuckGo]
        );

        let brave = WebSearchConfig {
            provider: "auto".to_string(),
            api_key: "key".to_string(),
            ..WebSearchConfig::default()
        };
        assert_eq!(
            provider_chain(&brave).expect("valid chain"),
            vec![SearchProvider::Brave, SearchProvider::DuckDuckGo]
        );

        // A search engine ID only exists for Google, so it selects Google.
        let google = WebSearchConfig {
            provider: "auto".to_string(),
            api_key: "key".to_string(),
            search_engine_id: "cx".to_string(),
            ..WebSearchConfig::default()
        };
        assert_eq!(
            provider_chain(&google).expect("valid chain"),
            vec![SearchProvider::Google, SearchProvider::DuckDuckGo]
        );

        // An explicit key-free choice never sends the query to a keyed API.
        let explicit = WebSearchConfig {
            provider: "duckduckgo".to_string(),
            api_key: "key".to_string(),
            ..WebSearchConfig::default()
        };
        assert_eq!(
            provider_chain(&explicit).expect("valid chain"),
            vec![SearchProvider::DuckDuckGo]
        );
    }

    #[test]
    fn a_fact_question_upgrades_the_card_but_a_miss_keeps_the_generic_one() {
        let generic = WebSearchAnswerCard {
            title: "Best result".to_string(),
            text: "Nicknamed Diesel, he is a 7-foot-1-inch center.".to_string(),
            sources: Vec::new(),
        };
        let items = vec![web_search_item(
            "Shaquille O'Neal",
            "https://en.wikipedia.org/wiki/Shaquille_O%27Neal",
            "Nicknamed Diesel, he is a 7-foot-1-inch center.",
        )
        .expect("valid result")];

        let mut answered = WebSearchResult {
            query: "how tall is shaq".to_string(),
            answer: None,
            summary: None,
            answer_card: Some(generic.clone()),
            items: items.clone(),
        };
        apply_contextual_answer(&mut answered, "how tall is shaq");
        assert_eq!(
            answered.answer_card.as_ref().map(|card| card.text.as_str()),
            Some("shaq — 7-foot-1-inch")
        );

        // Nothing in the results answers the question, so the provider's own
        // card has to survive rather than being replaced with nothing.
        let mut missed = WebSearchResult {
            query: "how tall is the eiffel tower".to_string(),
            answer: None,
            summary: None,
            answer_card: Some(generic.clone()),
            items: vec![web_search_item(
                "Eiffel Tower",
                "https://example.com/eiffel",
                "A wrought-iron lattice tower in Paris.",
            )
            .expect("valid result")],
        };
        apply_contextual_answer(&mut missed, "how tall is the eiffel tower");
        assert_eq!(missed.answer_card, Some(generic.clone()));

        // A query that asks for no particular fact is left alone entirely.
        let mut untouched = WebSearchResult {
            query: "shaquille o'neal".to_string(),
            answer: None,
            summary: None,
            answer_card: Some(generic.clone()),
            items,
        };
        apply_contextual_answer(&mut untouched, "shaquille o'neal");
        assert_eq!(untouched.answer_card, Some(generic));
    }

    #[test]
    fn brave_search_requires_a_configured_key() {
        let config = WebSearchConfig {
            provider: "brave".to_string(),
            ..WebSearchConfig::default()
        };
        assert!(search_brave("rust", &config)
            .expect_err("missing key rejected")
            .contains("requires 'api_key'"));
    }
}
