use crate::{calc::CalcEngine, config::WebSearchConfig};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_QUERY_CHARS: usize = 512;
const MAX_TITLE_CHARS: usize = 300;
const MAX_SNIPPET_CHARS: usize = 2_000;
const MIN_RESULTS: usize = 1;
const MAX_RESULTS: usize = 10;
const DUCKDUCKGO_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub markdown_link: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchResult {
    pub query: String,
    /// A provider-supplied direct answer, such as a unit conversion.
    pub answer: Option<String>,
    /// A descriptive result summary when no direct answer is available.
    pub summary: Option<String>,
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

pub fn search_web(query: &str, config: &WebSearchConfig) -> Result<WebSearchResult, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(WebSearchResult {
            query: String::new(),
            answer: None,
            summary: None,
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
            answer: Some(answer),
            summary: None,
            items: Vec::new(),
        });
    }

    match config.provider.trim().to_ascii_lowercase().as_str() {
        "google" => search_google(trimmed, config),
        "duckduckgo" => search_duckduckgo(trimmed, config.max_results),
        provider => Err(format!(
            "Unsupported web search provider '{provider}'; expected 'duckduckgo' or 'google'"
        )),
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
    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(DUCKDUCKGO_USER_AGENT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let mut answer: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut items: Vec<WebSearchItem> = Vec::new();
    let mut request_succeeded = false;
    let mut request_errors = Vec::new();

    // 1. First attempt DuckDuckGo Instant Answer API. If a natural-language
    // question has no answer, retry its subject (for example, "how tall is X"
    // becomes "X"), which is the form the topic API understands.
    for api_query in instant_answer_queries(query) {
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
                    (answer, summary) =
                        collect_duckduckgo_api_response(data, &mut items, max_results);
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

    // 2. If we need more items, query DuckDuckGo HTML Lite search
    if items.len() < max_results {
        let html_url = "https://html.duckduckgo.com/html/";
        let params = [("q", query), ("b", "")];

        match client.post(html_url).form(&params).send() {
            Ok(resp) if resp.status().is_success() => match resp.text() {
                Ok(html) => {
                    request_succeeded = true;
                    let parsed_items = parse_duckduckgo_html(&html, max_results);
                    for item in parsed_items {
                        if items.len() >= max_results {
                            break;
                        }
                        if !items.iter().any(|existing| existing.url == item.url) {
                            items.push(item);
                        }
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

    if !request_succeeded {
        return Err(if request_errors.is_empty() {
            "DuckDuckGo search failed".to_string()
        } else {
            request_errors.join("; ")
        });
    }

    if summary.is_none() && !items.is_empty() {
        summary = (!items[0].snippet.is_empty()).then(|| items[0].snippet.clone());
    }

    items.truncate(max_results);

    Ok(WebSearchResult {
        query: query.to_string(),
        answer,
        summary,
        items,
    })
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

    Ok(WebSearchResult {
        query: query.to_string(),
        answer: None,
        summary,
        items,
    })
}

fn collect_duckduckgo_api_response(
    data: DuckDuckGoApiResponse,
    items: &mut Vec<WebSearchItem>,
    max_results: usize,
) -> (Option<String>, Option<String>) {
    let answer = sanitized_optional_text(data.answer);
    let summary = sanitized_optional_text(data.abstract_text);

    if let (Some(url), Some(title)) = (
        data.abstract_url.filter(|url| !url.trim().is_empty()),
        data.heading
            .or(data.abstract_source)
            .filter(|title| !title.trim().is_empty()),
    ) {
        let snippet = summary.as_deref().or(answer.as_deref()).unwrap_or_default();
        if let Some(item) = web_search_item(&title, &url, snippet) {
            items.push(item);
        }
    }

    if let Some(topics) = data.related_topics {
        collect_ddg_topics(&topics, items, max_results);
    }

    (answer, summary)
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
            if u.is_empty() || t.is_empty() {
                continue;
            }
            let (title, snippet) = split_topic_text(t);
            if let Some(item) = web_search_item(&title, u, &snippet) {
                items.push(item);
            }
        }
    }
}

fn split_topic_text(text: &str) -> (String, String) {
    if let Some((title, rest)) = text.split_once(" - ") {
        (title.trim().to_string(), rest.trim().to_string())
    } else if let Some((title, rest)) = text.split_once(" — ") {
        (title.trim().to_string(), rest.trim().to_string())
    } else {
        (text.to_string(), String::new())
    }
}

pub fn parse_duckduckgo_html(html: &str, max_results: usize) -> Vec<WebSearchItem> {
    let mut results = Vec::new();
    let result_blocks: Vec<&str> = html.split("class=\"result ").collect();

    let link_re =
        Regex::new(r#"<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#).ok();
    let snippet_re = Regex::new(r#"<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>"#).ok();
    let snippet_tag_re =
        Regex::new(r#"class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</(?:a|div|span)>"#).ok();

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

                let snippet = if let Some(ref sre) = snippet_re {
                    sre.captures(block)
                        .and_then(|c| c.get(1))
                        .map(|m| strip_html_tags(m.as_str()))
                        .unwrap_or_default()
                } else if let Some(ref sre) = snippet_tag_re {
                    sre.captures(block)
                        .and_then(|c| c.get(1))
                        .map(|m| strip_html_tags(m.as_str()))
                        .unwrap_or_default()
                } else {
                    String::new()
                };

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
    raw.trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(max_chars)
        .collect()
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
          <a class="result__snippet">Official documentation and book for Rust.</a>
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

        let (answer, summary) = collect_duckduckgo_api_response(data, &mut items, 5);

        assert_eq!(answer.as_deref(), Some("35 cm = 13.7795 inches"));
        assert_eq!(
            summary.as_deref(),
            Some("A centimetre is a unit of length.")
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].snippet, "A centimetre is a unit of length.");
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
        let (title, snippet) = split_topic_text("Rust (programming language) - A systems language");
        assert_eq!(title, "Rust (programming language)");
        assert_eq!(snippet, "A systems language");
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
}
