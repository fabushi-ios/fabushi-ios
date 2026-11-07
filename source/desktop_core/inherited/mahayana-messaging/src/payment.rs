use crate::actor::ActorId;
use crate::conversation::{ConversationDestination, ConversationId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Money {
    pub currency: String,
    pub amount_minor: i64,
}

impl Money {
    pub fn new(currency: impl Into<String>, amount_minor: i64) -> Self {
        Self {
            currency: currency.into().to_uppercase(),
            amount_minor,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.currency.len() == 3
            && self.currency.bytes().all(|byte| byte.is_ascii_uppercase())
            && self.amount_minor >= 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceLine {
    pub label: String,
    pub amount: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InvoiceKind {
    OneTime,
    Subscription,
    Donation,
    DigitalGoods,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invoice {
    pub id: String,
    pub conversation_id: ConversationId,
    pub seller_id: ActorId,
    pub title: String,
    pub description: String,
    pub kind: InvoiceKind,
    pub currency: String,
    pub prices: Vec<PriceLine>,
    pub payload: String,
    pub provider_id: String,
    pub start_parameter: Option<String>,
    pub request_name: bool,
    pub request_email: bool,
    pub request_phone: bool,
    pub request_shipping_address: bool,
    pub flexible_shipping: bool,
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
}

impl Invoice {
    pub fn checked_total_minor(&self) -> Option<i64> {
        self.prices.iter().try_fold(0i64, |total, line| {
            total.checked_add(line.amount.amount_minor)
        })
    }

    pub fn total_minor(&self) -> i64 {
        self.checked_total_minor().unwrap_or(i64::MAX)
    }

    pub fn is_valid(&self) -> bool {
        !self.id.trim().is_empty()
            && !self.title.trim().is_empty()
            && self.currency.len() == 3
            && !self.prices.is_empty()
            && self
                .prices
                .iter()
                .all(|line| line.amount.currency == self.currency && line.amount.is_valid())
            && self.checked_total_minor().is_some_and(|total| total > 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShippingAddress {
    pub country_code: String,
    pub state: String,
    pub city: String,
    pub street_line1: String,
    pub street_line2: Option<String>,
    pub postal_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomerInfo {
    pub name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub shipping_address: Option<ShippingAddress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaymentStatus {
    Draft,
    Pending,
    RequiresAction,
    Authorized,
    Paid,
    Refunded,
    PartiallyRefunded,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentOrder {
    pub id: String,
    pub invoice_id: String,
    pub buyer_id: ActorId,
    pub status: PaymentStatus,
    pub amount: Money,
    pub customer: Option<CustomerInfo>,
    pub provider_payment_id: Option<String>,
    pub provider_receipt_url: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletBalance {
    pub asset: String,
    pub available_minor: i64,
    pub pending_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entitlement {
    pub id: String,
    pub owner_id: ActorId,
    pub product_id: String,
    pub order_id: String,
    pub starts_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
}

/// Source-neutral paid-send restriction state for a candidate destination (`SB-05` /
/// `initChatRestriction` / `refreshRestrictedRows` / `computeStarsCount` / `checkPaid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum PaidSendRestrictionState {
    Unresolved,
    Free,
    Paid { amount_per_message_minor: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaidDestinationCost {
    pub destination: ConversationDestination,
    pub amount_per_message_minor: i64,
    pub message_count: u32,
    pub total_amount_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaidSendPlan {
    pub outgoing_message_count: u32,
    pub destinations: Vec<PaidDestinationCost>,
    pub aggregate_amount_minor: i64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PaidSendValidationError {
    #[error("outgoing message count must be greater than zero")]
    InvalidOutgoingMessageCount,
    #[error("per-message paid amount {0} must be positive")]
    InvalidPerMessageAmount(i64),
    #[error("paid-send restriction for destination {0:?} is still unresolved")]
    RecipientRestrictionUnresolved(ConversationDestination),
    #[error("paid-send cost calculation overflowed")]
    CostOverflow,
    #[error("paid-send requires approval for {required_minor} minor units (approved: {approved_minor})")]
    ApprovalRequired {
        required_minor: i64,
        approved_minor: i64,
    },
    #[error("destination {0:?} was not part of the approved paid-send plan")]
    UnknownDestination(ConversationDestination),
    #[error("paid-send restriction changed during send for {destination:?} (expected {expected_per_message_minor}, live {live_per_message_minor})")]
    RestrictionChangedDuringSend {
        destination: ConversationDestination,
        expected_per_message_minor: i64,
        live_per_message_minor: i64,
    },
    #[error("insufficient remaining approved paid-send allowance for {destination:?} (required {required_minor}, remaining {remaining_approved_minor})")]
    InsufficientApprovedAllowance {
        destination: ConversationDestination,
        required_minor: i64,
        remaining_approved_minor: i64,
    },
}

impl PaidSendPlan {
    pub fn from_resolved(
        resolved_destinations: Vec<(ConversationDestination, PaidSendRestrictionState)>,
        outgoing_message_count: u32,
    ) -> Result<Self, PaidSendValidationError> {
        if outgoing_message_count == 0 {
            return Err(PaidSendValidationError::InvalidOutgoingMessageCount);
        }
        let count_i64 = i64::from(outgoing_message_count);
        let mut destinations = Vec::with_capacity(resolved_destinations.len());
        let mut aggregate_amount_minor = 0i64;
        for (destination, restriction) in resolved_destinations {
            let amount_per_message_minor = match restriction {
                PaidSendRestrictionState::Unresolved => {
                    return Err(PaidSendValidationError::RecipientRestrictionUnresolved(
                        destination,
                    ));
                }
                PaidSendRestrictionState::Free => 0,
                PaidSendRestrictionState::Paid {
                    amount_per_message_minor,
                } => {
                    if amount_per_message_minor <= 0 {
                        return Err(PaidSendValidationError::InvalidPerMessageAmount(
                            amount_per_message_minor,
                        ));
                    }
                    amount_per_message_minor
                }
            };
            let total_amount_minor = amount_per_message_minor
                .checked_mul(count_i64)
                .ok_or(PaidSendValidationError::CostOverflow)?;
            aggregate_amount_minor = aggregate_amount_minor
                .checked_add(total_amount_minor)
                .ok_or(PaidSendValidationError::CostOverflow)?;
            destinations.push(PaidDestinationCost {
                destination,
                amount_per_message_minor,
                message_count: outgoing_message_count,
                total_amount_minor,
            });
        }
        Ok(Self {
            outgoing_message_count,
            destinations,
            aggregate_amount_minor,
        })
    }

    pub const fn requires_payment(&self) -> bool {
        self.aggregate_amount_minor > 0
    }

    pub fn authorize_with_approval(
        &self,
        approved_minor: Option<i64>,
    ) -> Result<PaidSendApprovalConsumer, PaidSendValidationError> {
        let approved = approved_minor.unwrap_or(0).max(0);
        if self.aggregate_amount_minor > approved {
            return Err(PaidSendValidationError::ApprovalRequired {
                required_minor: self.aggregate_amount_minor,
                approved_minor: approved,
            });
        }
        let mut planned_per_message = BTreeMap::new();
        for item in &self.destinations {
            planned_per_message.insert(item.destination.clone(), item.amount_per_message_minor);
        }
        Ok(PaidSendApprovalConsumer {
            approved_minor: approved,
            remaining_approved_minor: approved,
            planned_per_message,
            consumed_by_destination: BTreeMap::new(),
        })
    }
}

/// Stateful per-destination approval consumer that bounds each destination/range send by
/// both the destination's planned per-message price and the remaining approved allowance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaidSendApprovalConsumer {
    pub approved_minor: i64,
    pub remaining_approved_minor: i64,
    pub planned_per_message: BTreeMap<ConversationDestination, i64>,
    pub consumed_by_destination: BTreeMap<ConversationDestination, i64>,
}

impl PaidSendApprovalConsumer {
    pub fn consume_for_destination(
        &mut self,
        destination: &ConversationDestination,
        live_restriction: PaidSendRestrictionState,
        message_count: u32,
    ) -> Result<i64, PaidSendValidationError> {
        if message_count == 0 {
            return Err(PaidSendValidationError::InvalidOutgoingMessageCount);
        }
        let Some(&expected_per_message_minor) = self.planned_per_message.get(destination) else {
            return Err(PaidSendValidationError::UnknownDestination(
                destination.clone(),
            ));
        };
        let live_per_message_minor = match live_restriction {
            PaidSendRestrictionState::Unresolved => {
                return Err(PaidSendValidationError::RecipientRestrictionUnresolved(
                    destination.clone(),
                ));
            }
            PaidSendRestrictionState::Free => 0,
            PaidSendRestrictionState::Paid {
                amount_per_message_minor,
            } => {
                if amount_per_message_minor <= 0 {
                    return Err(PaidSendValidationError::InvalidPerMessageAmount(
                        amount_per_message_minor,
                    ));
                }
                amount_per_message_minor
            }
        };
        if live_per_message_minor > expected_per_message_minor {
            return Err(PaidSendValidationError::RestrictionChangedDuringSend {
                destination: destination.clone(),
                expected_per_message_minor,
                live_per_message_minor,
            });
        }
        let required_minor = live_per_message_minor
            .checked_mul(i64::from(message_count))
            .ok_or(PaidSendValidationError::CostOverflow)?;
        if required_minor > self.remaining_approved_minor {
            return Err(PaidSendValidationError::InsufficientApprovedAllowance {
                destination: destination.clone(),
                required_minor,
                remaining_approved_minor: self.remaining_approved_minor,
            });
        }
        self.remaining_approved_minor -= required_minor;
        let entry = self
            .consumed_by_destination
            .entry(destination.clone())
            .or_insert(0);
        *entry = entry
            .checked_add(required_minor)
            .ok_or(PaidSendValidationError::CostOverflow)?;
        Ok(required_minor)
    }
}

/// Pure paid-send restriction resolver over candidate peer capability sets (`FBCP-ADR-004`).
pub fn resolve_paid_restriction_from_capabilities<'a, I>(
    conversation_exists: bool,
    candidate_capability_sets: I,
    ignore_paid_restrictions: bool,
    child_exempts_paid: bool,
) -> PaidSendRestrictionState
where
    I: IntoIterator<Item = Option<&'a [String]>>,
{
    if !conversation_exists {
        return PaidSendRestrictionState::Unresolved;
    }
    let mut per_message_minor = 0i64;
    for maybe_caps in candidate_capability_sets {
        let Some(capabilities) = maybe_caps else {
            return PaidSendRestrictionState::Unresolved;
        };
        let mut actor_cost = 0i64;
        for cap in capabilities {
            if cap == "paid_restriction_unresolved" {
                return PaidSendRestrictionState::Unresolved;
            }
            if let Some(raw) = cap
                .strip_prefix("paid_messages_minor:")
                .or_else(|| cap.strip_prefix("stars_per_message:"))
            {
                match raw.trim().parse::<i64>() {
                    Ok(amount) if amount > 0 => {
                        actor_cost = actor_cost.max(amount);
                    }
                    _ => return PaidSendRestrictionState::Unresolved,
                }
            } else if matches!(
                cap.as_str(),
                "paid_messages_required" | "stars_per_message_required"
            ) {
                actor_cost = actor_cost.max(1);
            }
        }
        per_message_minor = per_message_minor.saturating_add(actor_cost);
    }
    if ignore_paid_restrictions || child_exempts_paid || per_message_minor <= 0 {
        PaidSendRestrictionState::Free
    } else {
        PaidSendRestrictionState::Paid {
            amount_per_message_minor: per_message_minor,
        }
    }
}

#[cfg(test)]
mod paid_restriction_policy_tests {
    use super::*;

    #[test]
    fn resolve_paid_restriction_from_capabilities_covers_free_paid_exempt_and_unresolved() {
        assert_eq!(
            resolve_paid_restriction_from_capabilities(false, std::iter::empty(), false, false),
            PaidSendRestrictionState::Unresolved
        );
        assert_eq!(
            resolve_paid_restriction_from_capabilities(true, [None], false, false),
            PaidSendRestrictionState::Unresolved
        );
        let unresolved_caps = vec!["paid_restriction_unresolved".to_string()];
        assert_eq!(
            resolve_paid_restriction_from_capabilities(
                true,
                [Some(unresolved_caps.as_slice())],
                false,
                false
            ),
            PaidSendRestrictionState::Unresolved
        );
        let paid_caps = vec!["stars_per_message:25".to_string()];
        assert_eq!(
            resolve_paid_restriction_from_capabilities(
                true,
                [Some(paid_caps.as_slice())],
                false,
                false
            ),
            PaidSendRestrictionState::Paid {
                amount_per_message_minor: 25
            }
        );
        assert_eq!(
            resolve_paid_restriction_from_capabilities(
                true,
                [Some(paid_caps.as_slice())],
                true,
                false
            ),
            PaidSendRestrictionState::Free
        );
        assert_eq!(
            resolve_paid_restriction_from_capabilities(
                true,
                [Some(paid_caps.as_slice())],
                false,
                true
            ),
            PaidSendRestrictionState::Free
        );
    }
}
