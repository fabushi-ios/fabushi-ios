use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpgradeQuiesceSummary {
    pub quiescing: bool,
    pub running_turns: usize,
}

#[derive(Debug, Default)]
pub struct UpgradeRecreateResume {
    quiescing_for_upgrade: AtomicBool,
}

impl UpgradeRecreateResume {
    pub fn quiesce_for_upgrade(&self, running_turns: usize) -> UpgradeQuiesceSummary {
        self.quiescing_for_upgrade.store(true, Ordering::Release);
        UpgradeQuiesceSummary {
            quiescing: true,
            running_turns,
        }
    }

    pub fn is_quiescing_for_upgrade(&self) -> bool {
        self.quiescing_for_upgrade.load(Ordering::Acquire)
    }

    pub fn resume_after_recreate(&self) {
        self.quiescing_for_upgrade.store(false, Ordering::Release);
    }
}

pub fn build_upgrade_resume_prompt(source: &str) -> String {
    let tail = "You've been resumed with your full conversation intact. Continue exactly where you left off and finish what you were doing. If your previous step already completed an action, do NOT repeat it — just carry on from there. Remember: nothing reaches the user unless it's inside a SendMessage.";
    match source {
        "automation" => format!(
            "[A background system update restarted your environment and interrupted a scheduled routine run mid-task. {tail} This is still that routine's own run — nobody is waiting on it, so if its saved instruction says to stay quiet when there's nothing to report, ending with no SendMessage remains a valid outcome.]"
        ),
        "background-revival" => format!(
            "[A background system update restarted your environment and interrupted one of your background-work follow-ups mid-delivery. {tail} This follow-up was your own background wake — nobody is waiting on a reply, so if the results above your interruption point carry nothing genuinely new for the user, ending with no SendMessage remains a valid outcome.]"
        ),
        _ => format!(
            "[A background system update restarted your environment and interrupted you mid-task. {tail}]"
        ),
    }
}
