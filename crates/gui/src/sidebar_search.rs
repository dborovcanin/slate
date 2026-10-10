//! Searching the notes listed in the sidebar: titles by fuzzy match, then
//! note text through the database's full-text index.
use crate::note_view::NoteHost;
use crate::switcher::fuzzy;

const CONTENT_HITS: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub title: String,
    /// The matching text, for hits found in a note's body.
    pub snippet: Option<String>,
}

/// Hits for `query` among the notes of the working collection; empty for a
/// blank query.
pub fn search(host: &NoteHost, query: &str) -> Vec<Hit> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let mut titled: Vec<(i64, Hit)> = host
        .notes
        .iter()
        .filter_map(|n| {
            let title = if n.title.is_empty() {
                "Untitled"
            } else {
                n.title.as_str()
            };
            fuzzy(query, title).map(|score| {
                (
                    score,
                    Hit {
                        id: n.id.clone(),
                        title: title.to_string(),
                        snippet: None,
                    },
                )
            })
        })
        .collect();
    titled.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let mut hits: Vec<Hit> = titled.into_iter().map(|(_, hit)| hit).collect();
    let collection = host.working.as_ref().map(|(id, _)| id.as_str());
    if let Ok(found) = host
        .db
        .search_notes_content_filtered(query, CONTENT_HITS, collection)
    {
        for result in found {
            if hits.iter().any(|h| h.id == result.id) {
                continue;
            }
            let title = if result.title.is_empty() {
                "Untitled".to_string()
            } else {
                result.title
            };
            hits.push(Hit {
                id: result.id,
                title,
                snippet: Some(result.snippet.trim().to_string()).filter(|s| !s.is_empty()),
            });
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_match_fuzzily_and_bodies_through_the_index() {
        let dir = std::env::temp_dir().join(format!("slate-gui-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = app_core::storage::Db::open(dir.join("notes.db")).unwrap();
        for (id, body) in [
            ("a", "# Lisbon trip\nflights"),
            ("b", "# Budget\nthe lisbon hotel costs 300"),
            ("c", "# Garden\nroses"),
        ] {
            db.create_note_with_context(id, Default::default(), None, None)
                .unwrap();
            db.save_note(id, body).unwrap();
        }
        let host = NoteHost::open(db, Some("c")).unwrap();
        let hits = search(&host, "lisb");
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "{hits:?}");
        assert!(hits[0].snippet.is_none() && hits[1].snippet.is_some());
        assert!(search(&host, "  ").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
