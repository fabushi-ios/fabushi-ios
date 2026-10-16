use prost::{Message, Oneof};

#[derive(Clone, PartialEq, Message)]
pub struct Workflow {
    #[prost(message, repeated, tag = "10")]
    pub triggers: Vec<Trigger>,
    #[prost(message, repeated, tag = "3")]
    pub prompts: Vec<Prompt>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Prompt {
    #[prost(string, tag = "1")]
    pub prompt: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct Trigger {
    #[prost(oneof = "trigger::Trigger", tags = "1, 2, 7, 9, 13, 15, 16, 18, 19, 20")]
    pub trigger: Option<trigger::Trigger>,
}

pub mod trigger {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Trigger {
        #[prost(message, tag = "1")]
        Cron(CronTrigger),
        #[prost(message, tag = "2")]
        Git(GitTrigger),
        #[prost(message, tag = "7")]
        SlackTrigger(SlackTrigger),
        #[prost(message, tag = "9")]
        Linear(LinearTrigger),
        #[prost(message, tag = "13")]
        Pagerduty(PagerDutyTrigger),
        #[prost(message, tag = "15")]
        Sentry(SentryTrigger),
        #[prost(message, tag = "16")]
        MicrosoftTeamsTrigger(MicrosoftTeamsTrigger),
        #[prost(message, tag = "18")]
        SlackReactionAdded(SlackReactionAddedTrigger),
        #[prost(message, tag = "19")]
        SlackMention(SlackMentionTrigger),
        #[prost(message, tag = "20")]
        SlackAnyReactionAdded(SlackAnyReactionAddedTrigger),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct CronTrigger {
    #[prost(string, tag = "1")]
    pub cron: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct SlackTrigger {
    #[prost(string, tag = "1")]
    pub channel: String,
    #[prost(string, tag = "3")]
    pub message_contains: String,
    #[prost(bool, tag = "4")]
    pub message_contains_is_regex: bool,
    #[prost(string, repeated, tag = "5")]
    pub channels: Vec<String>,
    #[prost(bool, tag = "6")]
    pub block_unauthenticated_slack_users: bool,
    #[prost(bool, optional, tag = "9")]
    pub top_level_only: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SlackMentionTrigger {
    #[prost(string, tag = "1")]
    pub channel: String,
    #[prost(string, repeated, tag = "2")]
    pub channels: Vec<String>,
    #[prost(bool, tag = "3")]
    pub block_unauthenticated_slack_users: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct SlackReactionAddedTrigger {
    #[prost(string, tag = "1")]
    pub channel: String,
    #[prost(string, tag = "3")]
    pub emoji_name: String,
    #[prost(string, repeated, tag = "5")]
    pub channels: Vec<String>,
    #[prost(bool, tag = "6")]
    pub block_unauthenticated_slack_users: bool,
    #[prost(bool, tag = "9")]
    pub only_owner_reactions: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct SlackAnyReactionAddedTrigger {
    #[prost(string, tag = "1")]
    pub channel: String,
    #[prost(string, repeated, tag = "2")]
    pub channels: Vec<String>,
    #[prost(bool, tag = "3")]
    pub block_unauthenticated_slack_users: bool,
    #[prost(bool, tag = "4")]
    pub only_owner_reactions: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct GitTrigger {
    #[prost(oneof = "git_trigger::Event", tags = "1, 4, 8, 9, 10, 12, 13")]
    pub event: Option<git_trigger::Event>,
    #[prost(string, repeated, tag = "3")]
    pub user_allowlist: Vec<String>,
}

pub mod git_trigger {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        PullRequest(GitPullRequestEvent),
        #[prost(message, tag = "4")]
        CiCompleted(GitCiCompletedEvent),
        #[prost(message, tag = "8")]
        PullRequestReviewComment(GitPullRequestReviewCommentEvent),
        #[prost(message, tag = "9")]
        PullRequestReview(GitPullRequestReviewEvent),
        #[prost(message, tag = "10")]
        ReviewThread(GitReviewThreadEvent),
        #[prost(message, tag = "12")]
        PullRequestReviewRequested(GitPullRequestReviewRequestedEvent),
        #[prost(message, tag = "13")]
        IssueAssigned(GitIssueAssignedEvent),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct GitPullRequestEvent {
    #[prost(string, repeated, tag = "2")]
    pub repos: Vec<String>,
    #[prost(int32, tag = "5")]
    pub pr_action: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct GitPullRequestReviewRequestedEvent {
    #[prost(string, repeated, tag = "1")]
    pub repos: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GitIssueAssignedEvent {
    #[prost(string, repeated, tag = "1")]
    pub repos: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GitPullRequestReviewCommentEvent {
    #[prost(string, repeated, tag = "1")]
    pub repos: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GitPullRequestReviewEvent {
    #[prost(string, repeated, tag = "1")]
    pub repos: Vec<String>,
    #[prost(bool, tag = "2")]
    pub on_approved: bool,
    #[prost(bool, tag = "3")]
    pub on_changes_requested: bool,
    #[prost(bool, tag = "4")]
    pub on_commented: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct GitReviewThreadEvent {
    #[prost(string, repeated, tag = "1")]
    pub repos: Vec<String>,
    #[prost(bool, tag = "2")]
    pub on_resolved: bool,
    #[prost(bool, tag = "3")]
    pub on_unresolved: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct GitCiCompletedEvent {
    #[prost(string, repeated, tag = "1")]
    pub repos: Vec<String>,
    #[prost(int32, tag = "2")]
    pub condition: i32,
    #[prost(string, tag = "4")]
    pub branch: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct MicrosoftTeamsTrigger {
    #[prost(string, tag = "1")]
    pub tenant_id: String,
    #[prost(string, tag = "2")]
    pub team_id: String,
    #[prost(string, repeated, tag = "3")]
    pub team_ids: Vec<String>,
    #[prost(string, repeated, tag = "4")]
    pub channel_ids: Vec<String>,
    #[prost(string, tag = "5")]
    pub message_contains: String,
    #[prost(bool, tag = "6")]
    pub message_contains_is_regex: bool,
    #[prost(bool, tag = "7")]
    pub block_unauthenticated_teams_users: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct LinearTrigger {
    #[prost(oneof = "linear_trigger::Event", tags = "1, 2, 3")]
    pub event: Option<linear_trigger::Event>,
    #[prost(string, repeated, tag = "4")]
    pub project_ids: Vec<String>,
    #[prost(string, repeated, tag = "6")]
    pub team_ids: Vec<String>,
}

pub mod linear_trigger {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        IssueCreated(LinearIssueCreatedEvent),
        #[prost(message, tag = "2")]
        StatusChanged(LinearStatusChangedEvent),
        #[prost(message, tag = "3")]
        EndOfCycle(LinearEndOfCycleEvent),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct LinearIssueCreatedEvent {}

#[derive(Clone, PartialEq, Message)]
pub struct LinearStatusChangedEvent {
    #[prost(string, repeated, tag = "1")]
    pub status_ids: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LinearEndOfCycleEvent {
    #[prost(string, repeated, tag = "1")]
    pub cycle_ids: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SentryTrigger {
    #[prost(oneof = "sentry_trigger::Event", tags = "1, 2, 3, 4, 5, 6")]
    pub event: Option<sentry_trigger::Event>,
    #[prost(string, repeated, tag = "7")]
    pub project_ids: Vec<String>,
}

pub mod sentry_trigger {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        IssueCreated(SentryIssueCreatedEvent),
        #[prost(message, tag = "2")]
        IssueResolved(SentryIssueResolvedEvent),
        #[prost(message, tag = "3")]
        IssueAssigned(SentryIssueAssignedEvent),
        #[prost(message, tag = "4")]
        IssueArchived(SentryIssueArchivedEvent),
        #[prost(message, tag = "5")]
        IssueUnresolved(SentryIssueUnresolvedEvent),
        #[prost(message, tag = "6")]
        IssueAny(SentryIssueAnyEvent),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct SentryIssueCreatedEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct SentryIssueResolvedEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct SentryIssueAssignedEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct SentryIssueArchivedEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct SentryIssueUnresolvedEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct SentryIssueAnyEvent {}

#[derive(Clone, PartialEq, Message)]
pub struct PagerDutyTrigger {
    #[prost(oneof = "pager_duty_trigger::Event", tags = "1, 2, 3, 4, 5")]
    pub event: Option<pager_duty_trigger::Event>,
    #[prost(string, repeated, tag = "6")]
    pub service_ids: Vec<String>,
}

pub mod pager_duty_trigger {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Event {
        #[prost(message, tag = "1")]
        IncidentTriggered(PagerDutyIncidentTriggeredEvent),
        #[prost(message, tag = "2")]
        IncidentAcknowledged(PagerDutyIncidentAcknowledgedEvent),
        #[prost(message, tag = "3")]
        IncidentResolved(PagerDutyIncidentResolvedEvent),
        #[prost(message, tag = "4")]
        IncidentEscalated(PagerDutyIncidentEscalatedEvent),
        #[prost(message, tag = "5")]
        IncidentAny(PagerDutyIncidentAnyEvent),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct PagerDutyIncidentTriggeredEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct PagerDutyIncidentAcknowledgedEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct PagerDutyIncidentResolvedEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct PagerDutyIncidentEscalatedEvent {}
#[derive(Clone, PartialEq, Message)]
pub struct PagerDutyIncidentAnyEvent {}

#[derive(Clone, PartialEq, Message)]
pub struct ListSandAutomationsRequest {
    #[prost(string, tag = "1")]
    pub sand_agent_id: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ListAutomationsResponse {
    #[prost(message, repeated, tag = "1")]
    pub workflows: Vec<AutomationWithOwner>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AutomationWithOwner {
    #[prost(message, optional, tag = "1")]
    pub workflow: Option<Automation>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Automation {
    #[prost(bool, tag = "3")]
    pub enabled: bool,
    #[prost(string, optional, tag = "7")]
    pub description: Option<String>,
    #[prost(string, tag = "8")]
    pub automation_id: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct CreateAutomationRequest {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, optional, tag = "2")]
    pub workflow: Option<Workflow>,
    #[prost(string, optional, tag = "3")]
    pub description: Option<String>,
    #[prost(bool, optional, tag = "10")]
    pub enabled: Option<bool>,
    #[prost(string, optional, tag = "11")]
    pub sand_agent_id: Option<String>,
    #[prost(string, optional, tag = "12")]
    pub sand_automation_id: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CreateAutomationResponse {}

#[derive(Clone, PartialEq, Message)]
pub struct UpdateAutomationRequest {
    #[prost(string, optional, tag = "2")]
    pub name: Option<String>,
    #[prost(message, optional, tag = "3")]
    pub workflow: Option<Workflow>,
    #[prost(bool, optional, tag = "4")]
    pub enabled: Option<bool>,
    #[prost(string, optional, tag = "5")]
    pub description: Option<String>,
    #[prost(string, tag = "6")]
    pub automation_id: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct UpdateAutomationResponse {}

#[derive(Clone, PartialEq, Message)]
pub struct DeleteAutomationRequest {
    #[prost(string, tag = "2")]
    pub automation_id: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct DeleteAutomationResponse {}
