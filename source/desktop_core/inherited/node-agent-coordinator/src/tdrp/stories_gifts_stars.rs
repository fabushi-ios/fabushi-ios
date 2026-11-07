use crate::protocol::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum StoryPrivacy {
    Public,
    ContactsOnly,
    CloseFriends,
    SelectedUsers { user_ids: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoryItem {
    pub story_id: String,
    pub owner_id: String,
    pub media_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    pub privacy: StoryPrivacy,
    pub views_count: u32,
    pub viewers: Vec<String>,
    pub reactions: BTreeMap<String, String>,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub is_pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StarLedgerEntry {
    pub entry_id: String,
    pub user_id: String,
    pub amount: i64,
    pub reason: String,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GiftItem {
    pub gift_id: String,
    pub sender_id: String,
    pub recipient_id: String,
    pub title: String,
    pub stars_price: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub is_anonymous: bool,
    pub sent_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct StoriesGiftsStarsState {
    pub stories: BTreeMap<String, StoryItem>,
    pub star_balances: BTreeMap<String, u64>,
    pub star_ledgers: Vec<StarLedgerEntry>,
    pub received_gifts: BTreeMap<String, Vec<GiftItem>>,
    next_story_id: u32,
    next_ledger_id: u32,
    next_gift_id: u32,
}

impl StoriesGiftsStarsState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn post_story(
        &mut self,
        owner_id: String,
        media_url: String,
        caption: Option<String>,
        privacy: StoryPrivacy,
        ttl_seconds: u64,
        now_ms: i64,
    ) -> Result<StoryItem, Failure> {
        if media_url.trim().is_empty() {
            return Err(Failure::new(
                "INVALID_MEDIA_URL",
                "story media url cannot be empty",
            ));
        }
        let ttl_seconds = if ttl_seconds == 0 { 86_400 } else { ttl_seconds };
        self.next_story_id += 1;
        let story_id = format!("story_{}", self.next_story_id);
        let expires_at_ms = now_ms + (ttl_seconds as i64 * 1000);

        let story = StoryItem {
            story_id: story_id.clone(),
            owner_id,
            media_url,
            caption,
            privacy,
            views_count: 0,
            viewers: Vec::new(),
            reactions: BTreeMap::new(),
            created_at_ms: now_ms,
            expires_at_ms,
            is_pinned: false,
        };
        self.stories.insert(story_id, story.clone());
        Ok(story)
    }

    pub fn view_story(
        &mut self,
        story_id: &str,
        viewer_id: String,
        now_ms: i64,
    ) -> Result<StoryItem, Failure> {
        let story = self
            .stories
            .get_mut(story_id)
            .ok_or_else(|| Failure::new("STORY_NOT_FOUND", format!("story {story_id} not found")))?;

        if now_ms > story.expires_at_ms && !story.is_pinned {
            return Err(Failure::new("STORY_EXPIRED", "story has expired"));
        }

        if !story.viewers.contains(&viewer_id) {
            story.viewers.push(viewer_id);
            story.views_count += 1;
        }
        Ok(story.clone())
    }

    pub fn react_story(
        &mut self,
        story_id: &str,
        user_id: String,
        reaction: String,
    ) -> Result<StoryItem, Failure> {
        let story = self
            .stories
            .get_mut(story_id)
            .ok_or_else(|| Failure::new("STORY_NOT_FOUND", format!("story {story_id} not found")))?;

        if reaction.is_empty() {
            story.reactions.remove(&user_id);
        } else {
            story.reactions.insert(user_id, reaction);
        }
        Ok(story.clone())
    }

    pub fn delete_story(&mut self, story_id: &str, owner_id: &str) -> Result<bool, Failure> {
        let story = self
            .stories
            .get(story_id)
            .ok_or_else(|| Failure::new("STORY_NOT_FOUND", format!("story {story_id} not found")))?;

        if story.owner_id != owner_id {
            return Err(Failure::new(
                "UNAUTHORIZED_STORY_DELETE",
                "only the owner can delete the story",
            ));
        }
        self.stories.remove(story_id);
        Ok(true)
    }

    pub fn list_active_stories(&self, owner_id: Option<&str>, now_ms: i64) -> Vec<StoryItem> {
        self.stories
            .values()
            .filter(|s| {
                if let Some(owner) = owner_id {
                    if s.owner_id != owner {
                        return false;
                    }
                }
                s.is_pinned || s.expires_at_ms > now_ms
            })
            .cloned()
            .collect()
    }

    pub fn get_star_balance(&self, user_id: &str) -> u64 {
        self.star_balances.get(user_id).copied().unwrap_or(0)
    }

    pub fn purchase_stars(
        &mut self,
        user_id: &str,
        amount: u64,
        reason: String,
        now_ms: i64,
    ) -> u64 {
        let balance = self.star_balances.entry(user_id.to_string()).or_insert(0);
        *balance += amount;
        self.next_ledger_id += 1;
        self.star_ledgers.push(StarLedgerEntry {
            entry_id: format!("ledger_{}", self.next_ledger_id),
            user_id: user_id.to_string(),
            amount: amount as i64,
            reason,
            timestamp_ms: now_ms,
        });
        *balance
    }

    pub fn transfer_stars(
        &mut self,
        from_id: &str,
        to_id: &str,
        amount: u64,
        reason: String,
        now_ms: i64,
    ) -> Result<(u64, u64), Failure> {
        let from_balance = self.get_star_balance(from_id);
        if from_balance < amount {
            return Err(Failure::new("INSUFFICIENT_STARS", "not enough stars balance"));
        }
        let new_from = from_balance - amount;
        self.star_balances.insert(from_id.to_string(), new_from);

        let to_balance = self.get_star_balance(to_id);
        let new_to = to_balance + amount;
        self.star_balances.insert(to_id.to_string(), new_to);

        self.next_ledger_id += 1;
        self.star_ledgers.push(StarLedgerEntry {
            entry_id: format!("ledger_{}", self.next_ledger_id),
            user_id: from_id.to_string(),
            amount: -(amount as i64),
            reason: format!("Transfer to {to_id}: {reason}"),
            timestamp_ms: now_ms,
        });

        self.next_ledger_id += 1;
        self.star_ledgers.push(StarLedgerEntry {
            entry_id: format!("ledger_{}", self.next_ledger_id),
            user_id: to_id.to_string(),
            amount: amount as i64,
            reason: format!("Transfer from {from_id}: {reason}"),
            timestamp_ms: now_ms,
        });

        Ok((new_from, new_to))
    }

    pub fn send_gift(
        &mut self,
        sender_id: &str,
        recipient_id: &str,
        title: String,
        stars_price: u64,
        message: Option<String>,
        is_anonymous: bool,
        now_ms: i64,
    ) -> Result<GiftItem, Failure> {
        let balance = self.get_star_balance(sender_id);
        if balance < stars_price {
            return Err(Failure::new(
                "INSUFFICIENT_STARS_FOR_GIFT",
                "not enough stars to purchase gift",
            ));
        }

        self.star_balances
            .insert(sender_id.to_string(), balance - stars_price);
        self.next_ledger_id += 1;
        self.star_ledgers.push(StarLedgerEntry {
            entry_id: format!("ledger_{}", self.next_ledger_id),
            user_id: sender_id.to_string(),
            amount: -(stars_price as i64),
            reason: format!("Gift: {title}"),
            timestamp_ms: now_ms,
        });

        self.next_gift_id += 1;
        let gift_id = format!("gift_{}", self.next_gift_id);
        let gift = GiftItem {
            gift_id,
            sender_id: sender_id.to_string(),
            recipient_id: recipient_id.to_string(),
            title,
            stars_price,
            message,
            is_anonymous,
            sent_at_ms: now_ms,
        };
        self.received_gifts
            .entry(recipient_id.to_string())
            .or_default()
            .push(gift.clone());
        Ok(gift)
    }

    pub fn list_received_gifts(&self, recipient_id: &str) -> Vec<GiftItem> {
        self.received_gifts
            .get(recipient_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn handle_route(&mut self, method: &str, args: &Value) -> Result<Value, Failure> {
        if let Some(op) = method.strip_prefix("stories.") {
            match op {
                "post" | "publish" => {
                    let owner_id = args
                        .get("ownerId")
                        .or_else(|| args.get("userId"))
                        .and_then(Value::as_str)
                        .unwrap_or("self_user")
                        .to_string();
                    let media_url = args
                        .get("mediaUrl")
                        .or_else(|| args.get("url"))
                        .and_then(Value::as_str)
                        .unwrap_or("data:text/plain;charset=utf-8,story")
                        .to_string();
                    let caption = args
                        .get("caption")
                        .or_else(|| args.get("text"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    let privacy: StoryPrivacy = match args.get("privacy") {
                        Some(Value::String(s)) => match s.as_str() {
                            "contacts" => StoryPrivacy::ContactsOnly,
                            "close_friends" | "closeFriends" => StoryPrivacy::CloseFriends,
                            _ => StoryPrivacy::Public,
                        },
                        Some(v) => serde_json::from_value(v.clone()).unwrap_or(StoryPrivacy::Public),
                        None => StoryPrivacy::Public,
                    };
                    let ttl_seconds = args
                        .get("ttlSeconds")
                        .or_else(|| args.get("expirePeriod"))
                        .and_then(Value::as_u64)
                        .unwrap_or(86_400);
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let story = self.post_story(
                        owner_id,
                        media_url,
                        caption,
                        privacy,
                        ttl_seconds,
                        now_ms,
                    )?;
                    Ok(json!({ "story": story }))
                }
                "view" => {
                    let story_id = args
                        .get("storyId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "storyId required"))?;
                    let viewer_id = args
                        .get("viewerId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "viewerId required"))?
                        .to_string();
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let story = self.view_story(story_id, viewer_id, now_ms)?;
                    Ok(json!({ "story": story }))
                }
                "react" => {
                    let story_id = args
                        .get("storyId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "storyId required"))?;
                    let user_id = args
                        .get("userId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "userId required"))?
                        .to_string();
                    let reaction = args
                        .get("reaction")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();

                    let story = self.react_story(story_id, user_id, reaction)?;
                    Ok(json!({ "story": story }))
                }
                "delete" => {
                    let story_id = args
                        .get("storyId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "storyId required"))?;
                    let owner_id = args
                        .get("ownerId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "ownerId required"))?;

                    let deleted = self.delete_story(story_id, owner_id)?;
                    Ok(json!({ "deleted": deleted }))
                }
                "listActive" | "list" => {
                    let owner_id = args
                        .get("ownerId")
                        .or_else(|| args.get("userId"))
                        .and_then(Value::as_str);
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
                    let stories = self.list_active_stories(owner_id, now_ms);
                    Ok(json!({ "stories": stories }))
                }
                other => Err(Failure::new(
                    "UNKNOWN_STORY_METHOD",
                    format!("unrecognized story method {other}"),
                )),
            }
        } else if let Some(op) = method.strip_prefix("stars.") {
            match op {
                "getBalance" | "status" | "balance" => {
                    let user_id = args
                        .get("userId")
                        .or_else(|| args.get("ownerId"))
                        .and_then(Value::as_str)
                        .unwrap_or("self_user");
                    let balance = self.get_star_balance(user_id);
                    Ok(json!({ "balance": balance }))
                }
                "purchase" => {
                    let user_id = args
                        .get("userId")
                        .or_else(|| args.get("ownerId"))
                        .and_then(Value::as_str)
                        .unwrap_or("self_user");
                    let amount = args
                        .get("amount")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "amount required"))?;
                    let reason = args
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("In-app stars purchase")
                        .to_string();
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let new_balance = self.purchase_stars(user_id, amount, reason, now_ms);
                    Ok(json!({ "balance": new_balance }))
                }
                "transfer" => {
                    let from_id = args
                        .get("fromId")
                        .or_else(|| args.get("userId"))
                        .and_then(Value::as_str)
                        .unwrap_or("self_user");
                    let to_id = args
                        .get("toId")
                        .or_else(|| args.get("recipientId"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "toId required"))?;
                    let amount = args
                        .get("amount")
                        .or_else(|| args.get("starsPrice"))
                        .and_then(Value::as_u64)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "amount required"))?;
                    let reason = args
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("Peer transfer")
                        .to_string();
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let (from_bal, to_bal) =
                        self.transfer_stars(from_id, to_id, amount, reason, now_ms)?;
                    Ok(json!({ "fromBalance": from_bal, "toBalance": to_bal }))
                }
                other => Err(Failure::new(
                    "UNKNOWN_STARS_METHOD",
                    format!("unrecognized stars method {other}"),
                )),
            }
        } else if let Some(op) = method.strip_prefix("gifts.") {
            match op {
                "send" => {
                    let sender_id = args
                        .get("senderId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "senderId required"))?;
                    let recipient_id = args
                        .get("recipientId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "recipientId required"))?;
                    let title = args
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Virtual Gift")
                        .to_string();
                    let stars_price = args
                        .get("starsPrice")
                        .and_then(Value::as_u64)
                        .unwrap_or(25);
                    let message = args
                        .get("message")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    let is_anonymous = args
                        .get("isAnonymous")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let now_ms = args
                        .get("nowMs")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let gift = self.send_gift(
                        sender_id,
                        recipient_id,
                        title,
                        stars_price,
                        message,
                        is_anonymous,
                        now_ms,
                    )?;
                    Ok(json!({ "gift": gift }))
                }
                "listReceived" => {
                    let recipient_id = args
                        .get("recipientId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Failure::new("MISSING_ARGUMENT", "recipientId required"))?;
                    let gifts = self.list_received_gifts(recipient_id);
                    Ok(json!({ "gifts": gifts }))
                }
                other => Err(Failure::new(
                    "UNKNOWN_GIFTS_METHOD",
                    format!("unrecognized gifts method {other}"),
                )),
            }
        } else {
            Err(Failure::new(
                "UNKNOWN_METHOD",
                format!("unrecognized method {method}"),
            ))
        }
    }
}
