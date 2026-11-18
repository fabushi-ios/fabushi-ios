use crate::db::{ServerDatabase, SqliteResultExt};
use crate::search::tantivy_indexer::TantivyIndexer;
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

pub const DEFAULT_EMBEDDING_DIMS: usize = 64;
pub const RRF_K: f32 = 60.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDocument {
    pub message_id: String,
    pub chat_id: i64,
    pub content: String,
    pub embedding: Vec<f32>,
    pub linked_entities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchHit {
    pub message_id: String,
    pub chat_id: i64,
    pub content: String,
    pub rrf_score: f32,
    pub text_rank: Option<usize>,
    pub vector_rank: Option<usize>,
    pub graph_rank: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRelationEdge {
    pub src_entity: String,
    pub dst_entity: String,
    pub relation: String,
    pub chat_id: i64,
    pub doc_id: String,
    pub weight: f32,
}

#[derive(Debug)]
pub struct HybridMemoryEngine {
    docs: Mutex<Vec<MemoryDocument>>,
    db: Option<Arc<ServerDatabase>>,
    tantivy: Option<Weak<TantivyIndexer>>,
}

impl Default for HybridMemoryEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HybridMemoryEngine {
    pub fn new() -> Self {
        Self {
            docs: Mutex::new(Vec::new()),
            db: None,
            tantivy: None,
        }
    }

    pub fn with_db_and_tantivy(db: Arc<ServerDatabase>, tantivy: Weak<TantivyIndexer>) -> Self {
        let engine = Self {
            docs: Mutex::new(Vec::new()),
            db: Some(db),
            tantivy: Some(tantivy),
        };
        engine.hydrate_from_db();
        engine
    }

    fn hydrate_from_db(&self) {
        let Some(ref db) = self.db else { return };
        let loaded: Vec<MemoryDocument> = db
            .pool()
            .with_reader(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT doc_id, chat_id, content, embedding_json, linked_entities_json FROM memory_embeddings ORDER BY updated_at_ms DESC LIMIT 5000",
                    )
                    .map_db()?;
                let mut rows = stmt.query([]).map_db()?;
                let mut out = Vec::new();
                while let Some(r) = rows.next().map_db()? {
                    let emb_raw: String = r.get(3).map_db()?;
                    let ent_raw: String = r.get(4).map_db()?;
                    let embedding: Vec<f32> = serde_json::from_str(&emb_raw).unwrap_or_default();
                    let linked_entities: Vec<String> = serde_json::from_str(&ent_raw).unwrap_or_default();
                    out.push(MemoryDocument {
                        message_id: r.get(0).map_db()?,
                        chat_id: r.get(1).map_db()?,
                        content: r.get(2).map_db()?,
                        embedding,
                        linked_entities,
                    });
                }
                Ok(out)
            })
            .unwrap_or_default();
        if !loaded.is_empty() {
            let mut docs = self.docs.lock();
            *docs = loaded;
        }
    }

    /// Deterministic local normalized projection embedding for text when no external embedding vector is supplied.
    pub fn embed_text(text: &str, dims: usize) -> Vec<f32> {
        let dims = dims.max(8);
        let mut vec = vec![0.0f32; dims];
        let tokens = Self::tokenize(text);
        if tokens.is_empty() {
            return vec;
        }
        for tok in tokens {
            let mut hasher = Sha256::new();
            hasher.update(tok.as_bytes());
            let digest = hasher.finalize();
            let bucket = ((digest[0] as usize) | ((digest[1] as usize) << 8)) % dims;
            let sign = if digest[2] & 1 == 0 { 1.0f32 } else { -1.0f32 };
            vec[bucket] += sign;
            // Bigram / character trigram projection for fuzzy semantic overlap
            for win in tok.as_bytes().windows(3) {
                let mut h2 = Sha256::new();
                h2.update(win);
                let d2 = h2.finalize();
                let b2 = ((d2[0] as usize) | ((d2[1] as usize) << 8)) % dims;
                let s2 = if d2[2] & 1 == 0 { 0.5f32 } else { -0.5f32 };
                vec[b2] += s2;
            }
        }
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 && norm.is_finite() {
            for x in &mut vec {
                *x /= norm;
            }
        }
        vec
    }

    /// Extracts entity tokens (@mentions, #hashtags, capitalized terms, file identifiers).
    pub fn extract_entities(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for raw in text.split_whitespace() {
            let trimmed = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '@' && c != '#' && c != '-' && c != '.');
            if trimmed.len() < 2 {
                continue;
            }
            let is_tag = trimmed.starts_with('@') || trimmed.starts_with('#');
            let is_proper = trimmed.chars().next().is_some_and(|c| c.is_uppercase()) && trimmed.len() >= 3;
            if is_tag || is_proper {
                let norm = trimmed.trim_start_matches(['@', '#']).to_lowercase();
                if !norm.is_empty() && seen.insert(norm.clone()) {
                    out.push(norm);
                }
            }
        }
        out
    }

    pub fn index_document(&self, doc: MemoryDocument) {
        let mut docs = self.docs.lock();
        if let Some(existing) = docs
            .iter_mut()
            .find(|d| d.chat_id == doc.chat_id && d.message_id == doc.message_id)
        {
            *existing = doc.clone();
        } else {
            docs.push(doc.clone());
        }
        drop(docs);
        self.persist_document(&doc, "message");
    }

    pub fn index_message_or_file(
        &self,
        doc_id: &str,
        chat_id: i64,
        source_kind: &str,
        content: &str,
        explicit_entities: Vec<String>,
    ) {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut entities = Self::extract_entities(trimmed);
        for ent in explicit_entities {
            let norm = ent.trim().to_lowercase();
            if !norm.is_empty() && !entities.iter().any(|e| e == &norm) {
                entities.push(norm);
            }
        }
        let doc = MemoryDocument {
            message_id: doc_id.to_string(),
            chat_id,
            content: trimmed.to_string(),
            embedding: Self::embed_text(trimmed, DEFAULT_EMBEDDING_DIMS),
            linked_entities: entities,
        };
        {
            let mut docs = self.docs.lock();
            if let Some(existing) = docs
                .iter_mut()
                .find(|d| d.chat_id == doc.chat_id && d.message_id == doc.message_id)
            {
                *existing = doc.clone();
            } else {
                docs.push(doc.clone());
            }
        }
        self.persist_document(&doc, source_kind);
    }

    pub fn remove_document(&self, doc_id: &str) {
        self.docs.lock().retain(|d| d.message_id != doc_id);
        if let Some(ref db) = self.db {
            let _ = db.pool().with_transaction(|tx| {
                tx.execute("DELETE FROM memory_embeddings WHERE doc_id = ?1", params![doc_id])
                    .map_db()?;
                tx.execute("DELETE FROM memory_entity_edges WHERE doc_id = ?1", params![doc_id])
                    .map_db()?;
                Ok(())
            });
        }
    }

    pub fn remove_document_in_chat(&self, chat_id: i64, doc_id: &str) {
        self.docs
            .lock()
            .retain(|d| !(d.chat_id == chat_id && d.message_id == doc_id));
        if let Some(ref db) = self.db {
            let _ = db.pool().with_transaction(|tx| {
                tx.execute(
                    "DELETE FROM memory_embeddings WHERE chat_id = ?1 AND doc_id = ?2",
                    params![chat_id, doc_id],
                )
                .map_db()?;
                tx.execute(
                    "DELETE FROM memory_entity_edges WHERE chat_id = ?1 AND doc_id = ?2",
                    params![chat_id, doc_id],
                )
                .map_db()?;
                Ok(())
            });
        }
    }

    fn persist_document(&self, doc: &MemoryDocument, source_kind: &str) {
        let Some(ref db) = self.db else { return };
        let now_ms = Utc::now().timestamp_millis();
        let emb_json = serde_json::to_string(&doc.embedding).unwrap_or_else(|_| "[]".into());
        let ent_json = serde_json::to_string(&doc.linked_entities).unwrap_or_else(|_| "[]".into());
        let _ = db.pool().with_transaction(|tx| {
            tx.execute(
                r#"INSERT INTO memory_embeddings (
                       doc_id, chat_id, source_kind, content, dims, embedding_json, linked_entities_json, updated_at_ms
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                   ON CONFLICT(chat_id, doc_id) DO UPDATE SET
                       source_kind = excluded.source_kind,
                       content = excluded.content,
                       dims = excluded.dims,
                       embedding_json = excluded.embedding_json,
                       linked_entities_json = excluded.linked_entities_json,
                       updated_at_ms = excluded.updated_at_ms"#,
                params![
                    doc.message_id,
                    doc.chat_id,
                    source_kind,
                    doc.content,
                    doc.embedding.len() as i64,
                    emb_json,
                    ent_json,
                    now_ms,
                ],
            )
            .map_db()?;

            for ent in &doc.linked_entities {
                let norm = ent.trim().to_lowercase();
                if norm.is_empty() {
                    continue;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO memory_entities (entity_id, name, entity_type) VALUES (?1, ?2, 'entity')",
                    params![norm, ent],
                )
                .map_db()?;
                tx.execute(
                    r#"INSERT OR REPLACE INTO memory_entity_edges (src_entity, dst_entity, relation, chat_id, doc_id, weight)
                       VALUES (?1, ?2, 'mentions', ?3, ?4, 1.0)"#,
                    params![norm, doc.message_id, doc.chat_id, doc.message_id],
                )
                .map_db()?;
            }
            Ok(())
        });
    }

    pub fn compute_rrf_score(ranks: &[Option<usize>], k: f32) -> f32 {
        let mut score = 0.0f32;
        for &r in ranks {
            if let Some(rank) = r {
                score += 1.0 / (k + rank as f32);
            }
        }
        score
    }

    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        if a.iter().any(|x| !x.is_finite()) || b.iter().any(|x| !x.is_finite()) {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            let sim = dot / (norm_a * norm_b);
            if sim.is_finite() {
                sim
            } else {
                0.0
            }
        }
    }

    fn tokenize(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
            .collect()
    }

    pub fn search(
        &self,
        query_text: &str,
        query_embedding: Option<&[f32]>,
        allowed_chat_ids: &[i64],
        entity_hint: Option<&str>,
    ) -> Vec<HybridSearchHit> {
        self.search_with_limit(query_text, query_embedding, allowed_chat_ids, entity_hint, 50)
    }

    pub fn search_with_limit(
        &self,
        query_text: &str,
        query_embedding: Option<&[f32]>,
        allowed_chat_ids: &[i64],
        entity_hint: Option<&str>,
        limit: usize,
    ) -> Vec<HybridSearchHit> {
        if limit == 0 || allowed_chat_ids.is_empty() {
            return Vec::new();
        }
        let allowed_set: HashSet<i64> = allowed_chat_ids.iter().copied().collect();

        let lock = self.docs.lock();
        let filtered: Vec<MemoryDocument> = lock
            .iter()
            .filter(|d| allowed_set.contains(&d.chat_id))
            .cloned()
            .collect();
        drop(lock);

        if filtered.is_empty() {
            // Still check permission-filtered Tantivy if attached
            if let Some(t_weak) = &self.tantivy {
                if let Some(tantivy) = t_weak.upgrade() {
                    if let Ok(t_hits) = tantivy.search_hits_for_chats(query_text, allowed_chat_ids, limit) {
                        return t_hits
                            .into_iter()
                            .enumerate()
                            .filter_map(|(idx, h)| {
                                let cid = h.chat_id.parse::<i64>().ok()?;
                                if !allowed_set.contains(&cid) {
                                    return None;
                                }
                                let rank = Some(idx + 1);
                                Some(HybridSearchHit {
                                    message_id: h.message_id,
                                    chat_id: cid,
                                    content: h.snippet_html,
                                    rrf_score: Self::compute_rrf_score(&[rank, None, None], RRF_K),
                                    text_rank: rank,
                                    vector_rank: None,
                                    graph_rank: None,
                                })
                            })
                            .take(limit)
                            .collect();
                    }
                }
            }
            return Vec::new();
        }

        // Channel 1: Full-text search (permission-filtered Tantivy + tokenized term frequency)
        let mut text_ranks: HashMap<(i64, String), usize> = HashMap::new();
        let mut tantivy_candidates: Vec<(i64, String, String, usize)> = Vec::new();
        if let Some(t_weak) = &self.tantivy {
            if let Some(tantivy) = t_weak.upgrade() {
                if let Ok(t_hits) = tantivy.search_hits_for_chats(query_text, allowed_chat_ids, limit * 3) {
                    for (rank, h) in t_hits.into_iter().enumerate() {
                        if let Ok(cid) = h.chat_id.parse::<i64>() {
                            if allowed_set.contains(&cid) {
                                let composite_id = format!("{}:{}", cid, h.message_id);
                                text_ranks.entry((cid, h.message_id.clone())).or_insert(rank + 1);
                                text_ranks.entry((cid, composite_id)).or_insert(rank + 1);
                                tantivy_candidates.push((cid, h.message_id, h.snippet_html, rank + 1));
                            }
                        }
                    }
                }
            }
        }
        let q_terms = Self::tokenize(query_text);
        let mut text_scored: Vec<(&MemoryDocument, f32)> = filtered
            .iter()
            .map(|d| {
                if q_terms.is_empty() {
                    return (d, 0.0);
                }
                let doc_terms: HashSet<String> = Self::tokenize(&d.content).into_iter().collect();
                let matched = q_terms.iter().filter(|t| doc_terms.contains(*t)).count();
                let score = if matched > 0 {
                    matched as f32 / q_terms.len() as f32
                } else {
                    0.0
                };
                (d, score)
            })
            .filter(|(_, s)| !s.is_nan() && *s > 0.0)
            .collect();
        text_scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (rank, (d, _)) in text_scored.iter().enumerate() {
            text_ranks
                .entry((d.chat_id, d.message_id.clone()))
                .or_insert(rank + 1);
        }

        // Channel 2: Vector embedding cosine similarity (NaN-safe, zero-norm-safe)
        let mut vector_ranks: HashMap<(i64, String), usize> = HashMap::new();
        let auto_vec;
        let effective_q_vec: Option<&[f32]> = if let Some(v) = query_embedding {
            Some(v)
        } else if !q_terms.is_empty() && self.db.is_some() {
            auto_vec = Self::embed_text(query_text, DEFAULT_EMBEDDING_DIMS);
            Some(&auto_vec)
        } else {
            None
        };
        if let Some(q_vec) = effective_q_vec {
            let mut vec_scored: Vec<(&MemoryDocument, f32)> = filtered
                .iter()
                .map(|d| (d, Self::cosine_similarity(q_vec, &d.embedding)))
                .filter(|(_, s)| !s.is_nan() && *s > 0.1)
                .collect();
            vec_scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (rank, (d, _)) in vec_scored.iter().enumerate() {
                vector_ranks.insert((d.chat_id, d.message_id.clone()), rank + 1);
            }
        }

        // Channel 3: Entity Relationship Graph (direct + 1-hop entity expansion)
        let mut graph_ranks: HashMap<(i64, String), usize> = HashMap::new();
        let mut query_entities: Vec<String> = Vec::new();
        if let Some(entity) = entity_hint {
            let trimmed = entity.trim().to_lowercase();
            if !trimmed.is_empty() {
                query_entities.push(trimmed);
            }
        }
        if query_entities.is_empty() && self.db.is_some() {
            query_entities = Self::extract_entities(query_text);
        }
        if !query_entities.is_empty() {
            // Collect 1-hop co-occurring entities from authorized docs
            let mut expanded_entities: HashSet<String> = query_entities.iter().cloned().collect();
            for d in &filtered {
                let has_seed = d
                    .linked_entities
                    .iter()
                    .any(|e| query_entities.iter().any(|q| e.eq_ignore_ascii_case(q)));
                if has_seed {
                    for e in &d.linked_entities {
                        expanded_entities.insert(e.to_lowercase());
                    }
                }
            }

            let mut graph_scored: Vec<(&MemoryDocument, f32)> = filtered
                .iter()
                .map(|d| {
                    let direct_hits = d
                        .linked_entities
                        .iter()
                        .filter(|e| query_entities.iter().any(|q| e.eq_ignore_ascii_case(q)))
                        .count();
                    let hop_hits = d
                        .linked_entities
                        .iter()
                        .filter(|e| expanded_entities.contains(&e.to_lowercase()))
                        .count();
                    let score = (direct_hits as f32) + (hop_hits.saturating_sub(direct_hits) as f32) * 0.5;
                    (d, score)
                })
                .filter(|(_, s)| !s.is_nan() && *s > 0.0)
                .collect();
            graph_scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (rank, (d, _)) in graph_scored.iter().enumerate() {
                graph_ranks.insert((d.chat_id, d.message_id.clone()), rank + 1);
            }
        }

        // Deduplicated RRF fusion (k = 60.0)
        let mut hits = Vec::new();
        let mut seen_ids = HashSet::new();
        for d in &filtered {
            let doc_key = (d.chat_id, d.message_id.clone());
            if !seen_ids.insert(doc_key.clone()) {
                continue;
            }
            let bare_id = d
                .message_id
                .strip_prefix(&format!("{}:", d.chat_id))
                .unwrap_or(&d.message_id)
                .to_string();
            seen_ids.insert((d.chat_id, bare_id.clone()));
            seen_ids.insert((d.chat_id, format!("{}:{}", d.chat_id, bare_id)));
            let t_rank = text_ranks.get(&doc_key).copied();
            let v_rank = vector_ranks.get(&doc_key).copied();
            let g_rank = graph_ranks.get(&doc_key).copied();

            if t_rank.is_some() || v_rank.is_some() || g_rank.is_some() {
                let score = Self::compute_rrf_score(&[t_rank, v_rank, g_rank], RRF_K);
                hits.push(HybridSearchHit {
                    message_id: d.message_id.clone(),
                    chat_id: d.chat_id,
                    content: d.content.clone(),
                    rrf_score: score,
                    text_rank: t_rank,
                    vector_rank: v_rank,
                    graph_rank: g_rank,
                });
            }
        }

        // Include Tantivy-only hits that were not pre-populated in self.docs
        for (cid, raw_msg_id, snippet_html, rank) in tantivy_candidates {
            let bare_key = (cid, raw_msg_id.clone());
            let comp_key = (cid, format!("{}:{}", cid, raw_msg_id));
            if seen_ids.contains(&bare_key) || seen_ids.contains(&comp_key) {
                continue;
            }
            seen_ids.insert(bare_key);
            seen_ids.insert(comp_key);
            let t_rank = Some(rank);
            let score = Self::compute_rrf_score(&[t_rank, None, None], RRF_K);
            hits.push(HybridSearchHit {
                message_id: raw_msg_id,
                chat_id: cid,
                content: snippet_html,
                rrf_score: score,
                text_rank: t_rank,
                vector_rank: None,
                graph_rank: None,
            });
        }

        hits.sort_by(|a, b| b.rrf_score.total_cmp(&a.rrf_score));
        hits.truncate(limit);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_memory_rrf_acl_and_persistence() {
        let db = Arc::new(ServerDatabase::open_in_memory().unwrap());
        let idx = Arc::new(TantivyIndexer::in_ram().unwrap());
        let engine = HybridMemoryEngine::with_db_and_tantivy(db.clone(), Arc::downgrade(&idx));

        engine.index_message_or_file(
            "file:doc1",
            10,
            "file",
            "Rust Tokio architecture specification by @alice",
            vec!["tokio".into(), "alice".into()],
        );
        engine.index_message_or_file(
            "msg:99",
            20,
            "message",
            "Secret chat 20 tokio notes",
            vec!["tokio".into()],
        );

        let hits = engine.search("tokio architecture", None, &[10], Some("alice"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "file:doc1");
        assert!(hits[0].rrf_score > 0.0);

        // Verify SQLite reload hydration
        let engine2 = HybridMemoryEngine::with_db_and_tantivy(db, Arc::downgrade(&idx));
        let hits2 = engine2.search("architecture", None, &[10], Some("alice"));
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].message_id, "file:doc1");
    }

    #[test]
    fn test_hybrid_memory_cross_chat_doc_id_isolation_and_limit_zero() {
        let db = Arc::new(ServerDatabase::open_in_memory().unwrap());
        let idx = Arc::new(TantivyIndexer::in_ram().unwrap());
        let engine = HybridMemoryEngine::with_db_and_tantivy(db.clone(), Arc::downgrade(&idx));

        // Same bare doc_id across different chats must not overwrite each other
        engine.index_message_or_file("1", 10, "message", "Alpha quantum flux", vec![]);
        engine.index_message_or_file("1", 20, "message", "Beta nebula prism", vec![]);
        assert_eq!(engine.search("Alpha quantum", None, &[10], None).len(), 1);
        assert_eq!(engine.search("Beta nebula", None, &[20], None).len(), 1);
        let hits_both = engine.search("Alpha quantum", None, &[10, 20], None);
        assert!(hits_both.iter().any(|h| h.chat_id == 10));

        // Reload from SQLite and verify both distinct chats survived
        let engine2 = HybridMemoryEngine::with_db_and_tantivy(db.clone(), Arc::downgrade(&idx));
        assert_eq!(engine2.search("Alpha quantum", None, &[10], None).len(), 1);
        assert_eq!(engine2.search("Beta nebula", None, &[20], None).len(), 1);

        // Composite doc_ids isolate deletion across chats
        engine2.index_message_or_file("10:2", 10, "message", "Keep gamma_10", vec![]);
        engine2.index_message_or_file("20:2", 20, "message", "Delete delta_20", vec![]);
        engine2.remove_document("20:2");
        assert!(engine2
            .search("gamma_10", None, &[10], None)
            .iter()
            .any(|h| h.message_id == "10:2"));
        assert!(!engine2
            .search("delta_20", None, &[20], None)
            .iter()
            .any(|h| h.message_id == "20:2"));

        // limit = 0 must return 0 hits
        assert!(engine2
            .search_with_limit("Alpha quantum", None, &[10], None, 0)
            .is_empty());
    }
}
