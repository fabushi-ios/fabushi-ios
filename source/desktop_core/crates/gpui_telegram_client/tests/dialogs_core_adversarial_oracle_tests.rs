//! Blind C++ Adversarial Oracle Tests for Module 05: Dialogs Core Data & Sorting Math
//!
//! Mandate #18 Layer 3 Verification:
//! - Upstream C++ ground truth:
//!   - `Telegram/SourceFiles/dialogs/dialogs_entry.cpp:L33-L98` (Sort keys, tie-breaker bit shifts, BadgesForUnread)
//!   - `Telegram/SourceFiles/dialogs/dialogs_pinned_list.cpp:L20-L177` (Capacity limits, rotate-to-front, std::rotate)
//!   - `Telegram/SourceFiles/dialogs/dialogs_indexed_list.cpp:L220-L283` (Letter buckets, minimal bucket prefix search)
//!   - `Telegram/SourceFiles/dialogs/dialogs_main_list.cpp:L115-L169` (UnreadState delta tracking, cloud unread)
//!   - `Telegram/SourceFiles/dialogs/dialogs_common.h:L38-L93` (UnreadState arithmetic operators)

use std::collections::HashSet;
use gpui_telegram_client::dialogs::*;

// Simple deterministic PRNG for reproducible adversarial fuzzing without external rand crate
struct SimplePrng {
    state: u64,
}

impl SimplePrng {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_u32(&mut self) -> u32 {
        // Xorshift64star
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        ((self.state.wrapping_mul(0x2545F4914F6CDD1D)) >> 32) as u32
    }

    fn next_range(&mut self, min: usize, max: usize) -> usize {
        assert!(max >= min);
        if max == min {
            return min;
        }
        min + (self.next_u32() as usize % (max - min + 1))
    }

    fn next_i32_range(&mut self, min: i32, max: i32) -> i32 {
        assert!(max >= min);
        if max == min {
            return min;
        }
        let diff = (max as i64 - min as i64 + 1) as u32;
        min + (self.next_u32() % diff) as i32
    }
}

// ============================================================================
// Oracle 1: 64-bit Composite Sort Key Ordering & Tie-Breaker Shift Invariants
// ============================================================================
#[test]
fn test_oracle_1_composite_sort_key_10k_rapid_timestamps() {
    const RAPID_COUNT: usize = 10_000;
    let base_date = 1_730_800_000_i64;

    // 1A. Test 10,000 rapid timestamps with IDENTICAL date
    // Upstream C++: dialogs_entry.cpp:L37-42:
    // (uint64(date) << 32) | (++DialogsPosToTopShift)
    let mut identical_date_keys = Vec::with_capacity(RAPID_COUNT);
    for _ in 0..RAPID_COUNT {
        let key = dialog_pos_from_date(base_date);
        identical_date_keys.push(key);
    }

    assert_eq!(identical_date_keys.len(), RAPID_COUNT);

    // Verify strict monotonic increase and strict total order across 10,000 rapid calls
    for i in 0..RAPID_COUNT - 1 {
        let curr = identical_date_keys[i];
        let next = identical_date_keys[i + 1];

        assert!(
            curr < next,
            "Strict total ordering violation at index {}: curr={:#018x}, next={:#018x}",
            i,
            curr,
            next
        );

        // Verify date is preserved in the upper 32 bits
        assert_eq!(
            curr >> 32,
            base_date as u64,
            "Date upper 32 bits corrupted at index {}",
            i
        );

        // Verify shift counter increments strictly in lower 32 bits (at least by 1, accounting for thread concurrency)
        let shift_curr = (curr & 0xFFFFFFFF) as u32;
        let shift_next = (next & 0xFFFFFFFF) as u32;
        assert!(
            shift_next > shift_curr,
            "Tie-breaker shift must monotonically increase at index {}: curr={}, next={}",
            i,
            shift_curr,
            shift_next
        );
    }

    // 1B. Test 10,000 rapid timestamps with MONOTONIC increasing date
    let mut monotonic_date_keys = Vec::with_capacity(RAPID_COUNT);
    for i in 0..RAPID_COUNT {
        let date = base_date + (i as i64);
        monotonic_date_keys.push(dialog_pos_from_date(date));
    }

    for i in 0..RAPID_COUNT - 1 {
        assert!(
            monotonic_date_keys[i] < monotonic_date_keys[i + 1],
            "Monotonic date keys must strictly increase: k[{}]={:#018x} >= k[{}]={:#018x}",
            i,
            monotonic_date_keys[i],
            i + 1,
            monotonic_date_keys[i + 1]
        );
    }

    // 1C. Verify Strict Hierarchy Invariants:
    // fixed_on_top_dialog_pos(1) > fixed_on_top_dialog_pos(2)
    // > pinned_dialog_pos(1) > pinned_dialog_pos(N)
    // > unread_on_top_dialog_pos(d) > dialog_pos_from_date(d)
    let fixed_archive = fixed_on_top_dialog_pos(K_ARCHIVE_FIX_ON_TOP_INDEX);
    let fixed_promo = fixed_on_top_dialog_pos(K_TOP_PROMOTION_FIX_ON_TOP_INDEX);
    let pinned_1 = pinned_dialog_pos(1);
    let pinned_5 = pinned_dialog_pos(5);
    let pinned_10 = pinned_dialog_pos(10);
    let pinned_100 = pinned_dialog_pos(100);

    assert!(
        fixed_archive > fixed_promo,
        "Archive (fixed index 1) must exceed Promo (fixed index 2)"
    );
    assert!(
        fixed_promo > pinned_1,
        "Fixed-on-top promo must exceed pinned #1"
    );
    assert!(
        pinned_1 > pinned_5,
        "Pinned #1 must sort strictly above Pinned #5"
    );
    assert!(
        pinned_5 > pinned_10,
        "Pinned #5 must sort strictly above Pinned #10"
    );
    assert!(
        pinned_10 > pinned_100,
        "Pinned #10 must sort strictly above Pinned #100"
    );

    // Verify hierarchy holds against all 10,000 sampled dates
    for (idx, &date_pos) in identical_date_keys.iter().enumerate().step_by(10) {
        let unread_top = unread_on_top_dialog_pos(date_pos);

        assert!(
            pinned_100 > unread_top,
            "Pinned #100 ({:#018x}) must sort above unread-on-top ({:#018x}) at sample {}",
            pinned_100,
            unread_top,
            idx
        );
        assert!(
            unread_top > date_pos,
            "Unread-on-top ({:#018x}) must sort above date_pos ({:#018x}) at sample {}",
            unread_top,
            date_pos,
            idx
        );

        // Check high bit 63 invariant
        assert_ne!(
            unread_top & 0x8000000000000000,
            0,
            "Unread-on-top must have bit 63 set"
        );
        assert_eq!(
            date_pos & 0x8000000000000000,
            0,
            "Normal date pos must have bit 63 clear"
        );
    }

    // 1D. Boundary test: zero timestamp returns zero
    assert_eq!(dialog_pos_from_date(0), 0);
}

// ============================================================================
// Oracle 2: Pinned List Capacity Limits & Rotate-to-Front Invariants
// ============================================================================
#[test]
fn test_oracle_2_pinned_list_capacity_limits_and_rotate_to_front() {
    // 2A. Stress pinned list with 50 items under default limit (5)
    let mut list = DialogsPinnedList::new(0, 5);
    assert_eq!(list.limit, 5);
    assert_eq!(list.size(), 0);

    for i in 1..=50 {
        let key = DialogKey::History(i);
        list.set_pinned(key, true);

        // Capacity invariant: size <= 5 at every step
        assert!(
            list.size() <= 5,
            "Pinned list size {} exceeded limit 5 after inserting item {}",
            list.size(),
            i
        );

        // Rotate-to-front invariant: newly pinned item is ALWAYS at position 0
        assert_eq!(
            list.order()[0],
            key,
            "Item {} not rotated to position 0",
            i
        );
        assert_eq!(
            list.pinned_index(&key),
            1,
            "1-indexed pinned_index for newly pinned item must be 1"
        );

        // Uniqueness invariant: zero duplicate keys
        let set: HashSet<_> = list.order().iter().copied().collect();
        assert_eq!(set.len(), list.size(), "Duplicate keys detected in pinned list");

        // 1-indexed correspondence invariant
        for (idx, k) in list.order().iter().enumerate() {
            assert_eq!(
                list.pinned_index(k),
                idx + 1,
                "Pinned index mismatch for key {:?} at index {}",
                k,
                idx
            );
        }
    }

    // Final state of default list: exactly 5 items, latest first [50, 49, 48, 47, 46]
    assert_eq!(list.size(), 5);
    assert_eq!(
        list.order(),
        &[
            DialogKey::History(50),
            DialogKey::History(49),
            DialogKey::History(48),
            DialogKey::History(47),
            DialogKey::History(46),
        ]
    );

    // Evicted items 1..=45 must have pinned_index == 0
    for i in 1..=45 {
        assert_eq!(
            list.pinned_index(&DialogKey::History(i)),
            0,
            "Evicted item {} must report pinned_index 0",
            i
        );
    }

    // 2B. Rotate-to-front invariant under internal re-pinning
    // Re-pin item 48 (currently at index 2)
    list.set_pinned(DialogKey::History(48), true);
    assert_eq!(
        list.order(),
        &[
            DialogKey::History(48),
            DialogKey::History(50),
            DialogKey::History(49),
            DialogKey::History(47),
            DialogKey::History(46),
        ],
        "Re-pinning item 48 must rotate it to front and shift predecessors right"
    );
    assert_eq!(list.pinned_index(&DialogKey::History(48)), 1);
    assert_eq!(list.pinned_index(&DialogKey::History(50)), 2);
    assert_eq!(list.pinned_index(&DialogKey::History(49)), 3);
    assert_eq!(list.pinned_index(&DialogKey::History(47)), 4);
    assert_eq!(list.pinned_index(&DialogKey::History(46)), 5);

    // Re-pin item 46 (currently at the very end, index 4)
    list.set_pinned(DialogKey::History(46), true);
    assert_eq!(
        list.order(),
        &[
            DialogKey::History(46),
            DialogKey::History(48),
            DialogKey::History(50),
            DialogKey::History(49),
            DialogKey::History(47),
        ],
        "Re-pinning tail item 46 must rotate it to position 0"
    );

    // Re-pin item 46 (already at front) -> idempotent, no change
    list.set_pinned(DialogKey::History(46), true);
    assert_eq!(
        list.order(),
        &[
            DialogKey::History(46),
            DialogKey::History(48),
            DialogKey::History(50),
            DialogKey::History(49),
            DialogKey::History(47),
        ],
        "Re-pinning front item must be a no-op"
    );

    // 2C. Premium capacity expansion (5 -> 10)
    list.set_limit(10);
    assert_eq!(list.limit, 10);
    assert_eq!(list.size(), 5);

    for i in 51..=55 {
        list.set_pinned(DialogKey::History(i), true);
    }
    assert_eq!(list.size(), 10, "Premium capacity must accept 10 items");

    // 2D. Premium capacity contraction (10 -> 5)
    // Upstream: applyLimit pops from back until size <= limit
    list.set_limit(5);
    assert_eq!(list.size(), 5, "Downgrading limit to 5 must truncate to 5 items");
    assert_eq!(
        list.order(),
        &[
            DialogKey::History(55),
            DialogKey::History(54),
            DialogKey::History(53),
            DialogKey::History(52),
            DialogKey::History(51),
        ],
        "Downgrading limit must retain the topmost 5 items"
    );

    // 2E. Randomized Fuzzing Harness (500 steps)
    let mut prng = SimplePrng::new(0xCAFE_BABE_1234_5678);
    let mut fuzz_list = DialogsPinnedList::new(1, 5);

    for step in 0..500 {
        let op = prng.next_range(0, 4);
        match op {
            0 => {
                // Pin a random key between 1 and 30
                let id = prng.next_range(1, 30) as i64;
                fuzz_list.set_pinned(DialogKey::History(id), true);
            }
            1 => {
                // Unpin a random key
                let id = prng.next_range(1, 30) as i64;
                fuzz_list.set_pinned(DialogKey::History(id), false);
            }
            2 => {
                // Swap/Reorder two random existing elements
                if fuzz_list.size() >= 2 {
                    let idx1 = prng.next_range(0, fuzz_list.size() - 1);
                    let mut idx2 = prng.next_range(0, fuzz_list.size() - 1);
                    if idx1 == idx2 {
                        idx2 = (idx1 + 1) % fuzz_list.size();
                    }
                    let k1 = fuzz_list.order()[idx1];
                    let k2 = fuzz_list.order()[idx2];
                    fuzz_list.reorder(k1, k2);
                }
            }
            3 => {
                // Dynamic limit adjustment between 3 and 12
                let new_lim = prng.next_range(3, 12);
                fuzz_list.set_limit(new_lim);
            }
            _ => {
                // Add pinned at bottom
                let id = prng.next_range(1, 30) as i64;
                fuzz_list.add_pinned(DialogKey::History(id));
            }
        }

        // Assert all invariants hold at every step
        assert!(
            fuzz_list.size() <= fuzz_list.limit,
            "Fuzz step {}: size {} > limit {}",
            step,
            fuzz_list.size(),
            fuzz_list.limit
        );

        let unique: HashSet<_> = fuzz_list.order().iter().copied().collect();
        assert_eq!(
            unique.len(),
            fuzz_list.size(),
            "Fuzz step {}: duplicate entries detected",
            step
        );

        for (idx, k) in fuzz_list.order().iter().enumerate() {
            assert_eq!(
                fuzz_list.pinned_index(k),
                idx + 1,
                "Fuzz step {}: pinned_index wrong for key {:?} at index {}",
                step,
                k,
                idx
            );
        }
    }
}

// ============================================================================
// Oracle 3: Indexed List Minimal Bucket Prefix Search Combinations
// ============================================================================
#[test]
fn test_oracle_3_indexed_list_minimal_bucket_prefix_search_combinations() {
    let mut indexed_list = DialogsIndexedList::new(SortMode::Date, 0);

    // Build 2,000 realistic multi-word items
    let first_names = [
        "Alice", "Bob", "Charlie", "David", "Emma",
        "Frank", "Grace", "Henry", "Ivy", "Jack",
        "Kate", "Liam", "Mia", "Noah", "Olivia",
        "Peter", "Quinn", "Rose", "Sam", "Tom"
    ];
    let last_names = [
        "Adams", "Brown", "Clark", "Davis", "Evans",
        "Fisher", "Green", "Harris", "Irwin", "Jones",
        "King", "Lewis", "Miller", "Nelson", "Owens",
        "Parker", "Quigley", "Reed", "Scott", "Taylor"
    ];
    let tags = [
        "Alpha", "Beta", "Gamma", "Delta", "Echo",
        "Finance", "Gaming", "Mobile", "Security", "Support"
    ];

    let mut corpus: Vec<DialogEntryData> = Vec::with_capacity(2000);
    let mut id_counter = 1000_i64;

    // 20 * 20 * 5 = 2,000 distinct combinations
    for &fn_name in &first_names {
        for &ln_name in &last_names {
            for tag in tags.iter().take(5) {
                let title = format!("{} {} {}", fn_name, ln_name, tag);
                let entry = DialogEntryData {
                    id: id_counter,
                    title: title.clone(),
                    sort_key_name: title.to_lowercase(),
                    time_id: 1730000000 + id_counter,
                    ..Default::default()
                };
                indexed_list.add_to_end(entry.clone());
                corpus.push(entry);
                id_counter += 1;
            }
        }
    }

    assert_eq!(indexed_list.size(), 2000);
    assert_eq!(corpus.len(), 2000);

    // Reference Linear Scan Oracle
    // Upstream C++ specification: DialogsIndexedList::filtered sorts results by date descending (newest first)!
    let linear_oracle = |words: &[&str]| -> Vec<i64> {
        let non_empty: Vec<String> = words
            .iter()
            .map(|w| w.trim().to_lowercase())
            .filter(|w| !w.is_empty())
            .collect();
        if non_empty.is_empty() {
            return Vec::new();
        }

        let mut matches: Vec<&DialogEntryData> = corpus
            .iter()
            .filter(|entry| {
                let name_words = entry.name_words();
                non_empty.iter().all(|qw| {
                    name_words.iter().any(|nw| nw.starts_with(qw))
                })
            })
            .collect();

        // Sort descending by time_id (matching Telegram's SortMode::Date order)
        matches.sort_by(|a, b| b.time_id.cmp(&a.time_id));
        matches.iter().map(|e| e.id).collect()
    };

    // 3A. Test single-word prefix searches
    let single_test_words = ["ali", "bob", "charl", "dav", "em", "ada", "bro", "clar", "alp", "fin"];
    for &word in &single_test_words {
        let expected = linear_oracle(&[word]);
        let actual_rows = indexed_list.filtered_words(&[word.to_string()]);
        let actual_ids: Vec<i64> = actual_rows.iter().map(|r| r.entry.id).collect();

        assert_eq!(
            actual_ids.len(),
            expected.len(),
            "Single-word query '{}' count mismatch: expected {}, got {}",
            word,
            expected.len(),
            actual_ids.len()
        );
        assert_eq!(
            actual_ids,
            expected,
            "Single-word query '{}' result mismatch",
            word
        );
    }

    // 3B. Test multi-word prefix searches (2-word combinations)
    let multi_2_queries = [
        ["ali", "ada"],
        ["bob", "bro"],
        ["charl", "clar"],
        ["dav", "dav"],
        ["em", "ev"],
        ["fra", "fis"],
        ["gra", "gre"],
        ["hen", "har"],
        ["ivy", "irw"],
        ["jack", "jon"],
    ];

    for &[w1, w2] in &multi_2_queries {
        let expected = linear_oracle(&[w1, w2]);
        let actual_rows = indexed_list.filtered_words(&[w1.to_string(), w2.to_string()]);
        let actual_ids: Vec<i64> = actual_rows.iter().map(|r| r.entry.id).collect();

        assert_eq!(
            actual_ids.len(),
            expected.len(),
            "2-word query '{} {}' count mismatch: expected {}, got {}",
            w1,
            w2,
            expected.len(),
            actual_ids.len()
        );
        assert_eq!(
            actual_ids,
            expected,
            "2-word query '{} {}' result mismatch",
            w1,
            w2
        );
    }

    // 3C. Test multi-word prefix searches (3-word full intersections)
    let multi_3_queries = [
        ["ali", "ada", "alp"],
        ["bob", "bro", "bet"],
        ["charl", "clar", "gam"],
        ["dav", "dav", "del"],
        ["em", "ev", "ech"],
    ];

    for &[w1, w2, w3] in &multi_3_queries {
        let expected = linear_oracle(&[w1, w2, w3]);
        let actual_rows = indexed_list.filtered_words(&[w1.to_string(), w2.to_string(), w3.to_string()]);
        let actual_ids: Vec<i64> = actual_rows.iter().map(|r| r.entry.id).collect();

        assert_eq!(
            actual_ids,
            expected,
            "3-word query '{} {} {}' result mismatch",
            w1,
            w2,
            w3
        );
    }

    // 3D. Query Permutation Invariance & Minimal Bucket Optimization Test
    // For words with asymmetric bucket sizes:
    // "alpha" matches 400 entries (20 fn * 20 ln * 1 tag),
    // "alice" matches 100 entries (1 fn * 20 ln * 5 tags).
    // Whether query is ["alpha", "alice"] or ["alice", "alpha"],
    // the minimal bucket must be selected, producing identical results.
    let perm1 = indexed_list.filtered_words(&["alpha".to_string(), "alice".to_string()]);
    let perm2 = indexed_list.filtered_words(&["alice".to_string(), "alpha".to_string()]);
    let perm1_ids: Vec<i64> = perm1.iter().map(|r| r.entry.id).collect();
    let perm2_ids: Vec<i64> = perm2.iter().map(|r| r.entry.id).collect();

    assert_eq!(perm1_ids, perm2_ids, "Permuted search queries must yield identical result set");
    assert_eq!(perm1_ids, linear_oracle(&["alpha", "alice"]));

    // 3E. Disjoint / Negative Queries
    // "Alice" and "Bob" are both valid first names, but no entry has both
    let disjoint = indexed_list.filtered_words(&["alice".to_string(), "bob".to_string()]);
    assert!(disjoint.is_empty(), "Disjoint first names must return empty results");

    // Nonexistent prefix
    let nonexist = indexed_list.filtered_words(&["zztop".to_string()]);
    assert!(nonexist.is_empty(), "Nonexistent prefix must return empty results");

    // Valid word + nonexistent word
    let mixed_nonexist = indexed_list.filtered_words(&["alice".to_string(), "xyz999".to_string()]);
    assert!(mixed_nonexist.is_empty(), "Query containing nonexistent word must return empty results");

    // 3F. Empty & whitespace queries
    assert!(indexed_list.filtered_words(&[]).is_empty());
    assert!(indexed_list.filtered_words(&["   ".to_string()]).is_empty());
}

// ============================================================================
// Oracle 4: UnreadState Arithmetic & Badges Calculation Under Fuzzing
// ============================================================================
#[test]
fn test_oracle_4_unread_state_fuzzing_and_badge_arithmetic() {
    let mut prng = SimplePrng::new(0x1337_5EED_9876_5432);

    // 4A. Operator += and -= Round-Trip and Associativity Fuzzing (1,000 rounds)
    for _ in 0..1000 {
        let s1 = UnreadState {
            messages: prng.next_i32_range(0, 10_000),
            messages_muted: prng.next_i32_range(0, 10_000),
            chats: prng.next_i32_range(0, 500),
            chats_muted: prng.next_i32_range(0, 500),
            marks: prng.next_i32_range(0, 50),
            marks_muted: prng.next_i32_range(0, 50),
            reactions: prng.next_i32_range(0, 200),
            reactions_muted: prng.next_i32_range(0, 200),
            mentions: prng.next_i32_range(0, 100),
            polls: prng.next_i32_range(0, 50),
            polls_muted: prng.next_i32_range(0, 50),
            known: true,
        };

        let s2 = UnreadState {
            messages: prng.next_i32_range(0, 10_000),
            messages_muted: prng.next_i32_range(0, 10_000),
            chats: prng.next_i32_range(0, 500),
            chats_muted: prng.next_i32_range(0, 500),
            marks: prng.next_i32_range(0, 50),
            marks_muted: prng.next_i32_range(0, 50),
            reactions: prng.next_i32_range(0, 200),
            reactions_muted: prng.next_i32_range(0, 200),
            mentions: prng.next_i32_range(0, 100),
            polls: prng.next_i32_range(0, 50),
            polls_muted: prng.next_i32_range(0, 50),
            known: true,
        };

        // Round-trip invariant: (s1 + s2) - s2 == s1
        let mut accum = s1;
        accum += s2;
        assert_eq!(accum.messages, s1.messages + s2.messages);
        assert_eq!(accum.messages_muted, s1.messages_muted + s2.messages_muted);
        assert_eq!(accum.chats, s1.chats + s2.chats);
        assert_eq!(accum.reactions, s1.reactions + s2.reactions);
        assert_eq!(accum.mentions, s1.mentions + s2.mentions);
        assert_eq!(accum.polls, s1.polls + s2.polls);

        accum -= s2;
        assert_eq!(accum, s1, "UnreadState round-trip (s1 + s2) - s2 != s1");

        // Negative delta handling (when entry read decreases count)
        let delta = s2 - s1;
        let mut base = s1;
        base += delta;
        assert_eq!(base, s2, "UnreadState delta addition s1 + (s2 - s1) != s2");
    }

    // 4B. Badges Calculation Invariant Testing Under Fuzzing (1,000 states)
    for _ in 0..1000 {
        let msg_full = prng.next_i32_range(0, 500);
        let msg_muted = prng.next_i32_range(0, msg_full);
        let chat_full = prng.next_i32_range(0, 100);
        let chat_muted = prng.next_i32_range(0, chat_full);
        let marks_full = prng.next_i32_range(0, 10);
        let marks_muted = prng.next_i32_range(0, marks_full);
        let react_full = prng.next_i32_range(0, 20);
        let react_muted = prng.next_i32_range(0, react_full + 5);
        let polls_full = prng.next_i32_range(0, 10);
        let polls_muted = prng.next_i32_range(0, polls_full + 5);
        let mentions = prng.next_i32_range(0, 15);

        let state = UnreadState {
            messages: msg_full,
            messages_muted: msg_muted,
            chats: chat_full,
            chats_muted: chat_muted,
            marks: marks_full,
            marks_muted: marks_muted,
            reactions: react_full,
            reactions_muted: react_muted,
            mentions,
            polls: polls_full,
            polls_muted,
            known: true,
        };

        // Test with CountInBadge::Messages, IncludeInBadge::All
        let badges_all = badges_for_unread(
            &state,
            CountInBadge::Messages,
            IncludeInBadge::All,
            true,
            true,
        );
        let expected_counter_all = state.marks + state.messages;
        let is_mark_single_all = (expected_counter_all == 1) && (state.marks == 1);
        let expected_badge_all = if is_mark_single_all { 0 } else { expected_counter_all };

        assert_eq!(
            badges_all.unread_counter,
            expected_badge_all,
            "IncludeInBadge::All counter mismatch"
        );
        assert_eq!(badges_all.unread, expected_counter_all > 0);
        assert_eq!(badges_all.mention, mentions > 0);
        assert_eq!(badges_all.reaction, react_full > 0);
        assert_eq!(badges_all.reaction_muted, react_full <= react_muted);
        assert_eq!(badges_all.poll, polls_full > 0);
        assert_eq!(badges_all.poll_muted, polls_full <= polls_muted);

        // Test with CountInBadge::Messages, IncludeInBadge::Unmuted
        let badges_unmuted = badges_for_unread(
            &state,
            CountInBadge::Messages,
            IncludeInBadge::Unmuted,
            true,
            false,
        );
        let unmuted_counter = (state.marks - state.marks_muted) + (state.messages - state.messages_muted);
        let is_mark_single_unmuted = (unmuted_counter == 1) && ((state.marks - state.marks_muted) == 1);
        let expected_badge_unmuted = if is_mark_single_unmuted { 0 } else { unmuted_counter };

        assert_eq!(
            badges_unmuted.unread_counter,
            expected_badge_unmuted,
            "IncludeInBadge::Unmuted counter mismatch"
        );
        assert_eq!(badges_unmuted.unread, unmuted_counter > 0);

        // Test with CountInBadge::Chats
        let badges_chats = badges_for_unread(
            &state,
            CountInBadge::Chats,
            IncludeInBadge::All,
            false,
            true,
        );
        let expected_chats_counter = state.marks + state.chats;
        let is_mark_single_chat = (expected_chats_counter == 1) && (state.marks == 1);
        let expected_badge_chats = if is_mark_single_chat { 0 } else { expected_chats_counter };

        assert_eq!(
            badges_chats.unread_counter,
            expected_badge_chats,
            "CountInBadge::Chats counter mismatch"
        );
    }

    // 4C. Single-mark dot rule edge case
    // When exactly 1 mark exists and 0 messages/chats, counter == 1 && marks == 1 -> unread_counter == 0, unread == true
    let mark_only = UnreadState {
        marks: 1,
        known: true,
        ..Default::default()
    };
    let mark_badge = badges_for_unread(
        &mark_only,
        CountInBadge::Messages,
        IncludeInBadge::Default,
        false,
        false,
    );
    assert_eq!(mark_badge.unread_counter, 0, "Single mark must show as unread dot with counter 0");
    assert!(mark_badge.unread, "Single mark must report unread == true");

    // Empty state
    let empty_state = UnreadState::default();
    let empty_badge = badges_for_unread(
        &empty_state,
        CountInBadge::Default,
        IncludeInBadge::Default,
        false,
        false,
    );
    assert_eq!(empty_badge.unread_counter, 0);
    assert!(!empty_badge.unread);
    assert!(empty_badge.is_empty());
}
