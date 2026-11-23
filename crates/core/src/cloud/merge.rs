//! Deciding which side wins for settings sections. Pure functions, no network.

use std::collections::BTreeMap;

use crate::settings::{RemoteSection, Settings, SYNCED_SECTIONS};

/// What to do after comparing this computer's settings with the account's.
#[derive(Debug, Default, PartialEq)]
pub struct SectionPlan {
    /// Sections to take from the account.
    pub apply: BTreeMap<String, RemoteSection>,
    /// Sections to upload to the account.
    pub push: BTreeMap<String, RemoteSection>,
    /// Change times to record locally for sections that never had one (so they stay comparable).
    pub stamp: BTreeMap<String, i64>,
}

/// Last write wins, per section. On the first sign-in on a computer the rules differ: if the
/// account already has settings they are taken as they are (never-synced fields are untouched by
/// `Settings::apply_remote_sections`); if it has none, this computer's settings are uploaded.
pub fn merge_sections(local: &Settings, remote: &BTreeMap<String, RemoteSection>, first_merge: bool, now: i64) -> SectionPlan {
    let mine = local.synced_sections();
    let mut plan = SectionPlan::default();
    let time_of = |name: &str| local.section_times.get(name).copied().unwrap_or(0);

    let push_local = |plan: &mut SectionPlan, name: &str, value: &serde_json::Value| {
        let t = match time_of(name) {
            0 => {
                plan.stamp.insert(name.to_string(), now);
                now
            }
            t => t,
        };
        plan.push.insert(name.to_string(), RemoteSection { value: value.clone(), updated_at: t });
    };

    if first_merge {
        let account_has_settings = SYNCED_SECTIONS.iter().any(|n| remote.contains_key(*n));
        for name in SYNCED_SECTIONS {
            match remote.get(*name) {
                Some(r) if account_has_settings => {
                    plan.apply.insert((*name).to_string(), r.clone());
                }
                _ => {
                    if let Some(v) = mine.get(*name) {
                        push_local(&mut plan, name, v);
                    }
                }
            }
        }
        return plan;
    }

    for (name, value) in &mine {
        let lt = time_of(name);
        match remote.get(name) {
            Some(r) if r.updated_at > lt => {
                plan.apply.insert(name.clone(), r.clone());
            }
            Some(r) if r.updated_at < lt => {
                plan.push.insert(name.clone(), RemoteSection { value: value.clone(), updated_at: lt });
            }
            Some(_) => {}
            // The account doesn't have it yet: share it, but only if it was ever changed here.
            None if lt > 0 => {
                plan.push.insert(name.clone(), RemoteSection { value: value.clone(), updated_at: lt });
            }
            None => {}
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(v: serde_json::Value, t: i64) -> RemoteSection {
        RemoteSection { value: v, updated_at: t }
    }

    #[test]
    fn first_merge_with_empty_account_uploads_everything_and_stamps() {
        let s = Settings::default();
        let plan = merge_sections(&s, &BTreeMap::new(), true, 100);
        assert_eq!(plan.push.len(), SYNCED_SECTIONS.len());
        assert!(plan.apply.is_empty());
        assert_eq!(plan.stamp.len(), SYNCED_SECTIONS.len());
        assert!(plan.push.values().all(|p| p.updated_at == 100));
    }

    #[test]
    fn first_merge_takes_account_sections() {
        let s = Settings::default();
        let mut remote = BTreeMap::new();
        remote.insert("userName".to_string(), sec("Sarah".into(), 5));
        let plan = merge_sections(&s, &remote, true, 100);
        assert_eq!(plan.apply.len(), 1);
        // Sections the account lacks are shared from here.
        assert_eq!(plan.push.len(), SYNCED_SECTIONS.len() - 1);
        assert!(!plan.push.contains_key("userName"));
    }

    #[test]
    fn later_merges_are_last_write_wins_per_section() {
        let mut s = Settings::default();
        s.section_times.insert("userName".into(), 50);
        s.section_times.insert("theme".into(), 50);
        s.section_times.insert("sounds".into(), 50);
        s.section_times.insert("typingWpm".into(), 50);
        let mut remote = BTreeMap::new();
        remote.insert("userName".to_string(), sec("Remote".into(), 60)); // newer remote
        remote.insert("theme".to_string(), sec("dark".into(), 40)); // older remote
        remote.insert("sounds".to_string(), sec(serde_json::to_value(&s.sounds).unwrap(), 50)); // tie
        let plan = merge_sections(&s, &remote, false, 100);
        assert_eq!(plan.apply.keys().collect::<Vec<_>>(), vec!["userName"]);
        let mut pushed: Vec<_> = plan.push.keys().cloned().collect();
        pushed.sort();
        assert_eq!(pushed, vec!["theme", "typingWpm"]);
        assert!(plan.stamp.is_empty());
    }
}
