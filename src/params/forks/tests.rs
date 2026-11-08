use super::*;

fn scheduled(name: &str, height: u64) -> ScheduledFork {
    ScheduledFork { name: name.to_string(), height, epoch: None, least: Some(Version::new(0, 3, 0)) }
}

#[test]
fn a_fork_is_in_force_from_its_height_and_one_this_release_does_not_know_never_is() {
    let schedule = ForkSchedule { forks: vec![scheduled("test-fork", 10), scheduled("from-the-future", 5)], lead: None };

    assert_eq!(schedule.at(9), Forks::NONE);
    assert!(!schedule.at(9).has(&TEST_FORK));
    assert!(schedule.at(10).has(&TEST_FORK));
    assert_eq!(schedule.at(10_000).names(), ["test-fork"], "the unknown one is never in force");
    assert_eq!(schedule.unknown().map(|fork| fork.name.as_str()), Some("from-the-future"));
    assert_eq!(ForkSchedule::default().at(u64::MAX), Forks::NONE, "a chain that scheduled none runs as it always did");
}

#[test]
fn a_fork_is_scheduled_by_height_or_epoch_ahead_of_the_lead_and_moved_or_cancelled_until_it_passes() {
    let least = Version::new(0, 3, 0);
    let lead = ForkSchedule { forks: Vec::new(), lead: Some(100) };
    let schedule = |name: &str, at| ForkChange::Schedule { name: name.to_string(), at, least };

    // Ahead of the lead, by height or an epoch's first block; not before it, nor by epoch without finality.
    assert_eq!(schedule("test-fork", ForkAt::Height(150)).problem(&lead, 50, Some(10), None), None);
    assert!(schedule("test-fork", ForkAt::Height(149)).problem(&lead, 50, Some(10), None).unwrap().contains("lead"));
    assert_eq!(schedule("test-fork", ForkAt::Epoch(15)).problem(&lead, 50, Some(10), None), None);
    assert!(schedule("test-fork", ForkAt::Epoch(15)).problem(&lead, 50, None, None).unwrap().contains("finality"));
    assert!(schedule("Test", ForkAt::Height(500)).problem(&lead, 50, None, None).unwrap().contains("lowercase"));
    assert!(ForkChange::Schedule { name: "x".to_string(), at: ForkAt::Height(500), least: Version::default() }.problem(&lead, 50, None, None).unwrap().contains("0.0.0"));

    // One the release does not know may be scheduled: nodes that do not know it stop there.
    assert_eq!(schedule("from-the-future", ForkAt::Height(500)).problem(&lead, 50, None, None), None);

    let placed = schedule("test-fork", ForkAt::Epoch(20)).apply(&lead, Some(10));

    assert_eq!(placed.forks, [ScheduledFork { name: "test-fork".to_string(), height: 200, epoch: Some(20), least: Some(least) }]);

    // Moved, then cancelled, while it is ahead of the lead.
    let moved = schedule("test-fork", ForkAt::Height(300)).apply(&placed, Some(10));

    assert_eq!((moved.forks.len(), moved.forks[0].height, moved.forks[0].epoch), (1, 300, None));
    assert_eq!(ForkChange::Cancel { name: "test-fork".to_string() }.problem(&moved, 150, None, None), None);
    assert!(ForkChange::Cancel { name: "test-fork".to_string() }.apply(&moved, None).forks.is_empty());
    assert!(ForkChange::Cancel { name: "nothing".to_string() }.problem(&moved, 150, None, None).unwrap().contains("no fork"));

    // Within the lead, or passed, it is fixed.
    for height in [250, 300, 400] {
        assert!(ForkChange::Cancel { name: "test-fork".to_string() }.problem(&moved, height, None, None).unwrap().contains("passed"), "{height}");
        assert!(schedule("test-fork", ForkAt::Height(900)).problem(&moved, height, None, None).unwrap().contains("passed"), "{height}");
    }

    // The lead: governed, a block at least, a day of blocks unless set.
    assert!(ForkChange::Lead { blocks: 0 }.problem(&lead, 1, None, None).is_some());
    assert_eq!(ForkChange::Lead { blocks: 5 }.apply(&lead, None).lead(Some(5_000)), 5);
    assert_eq!(ForkSchedule::default().lead(Some(5_000)), 17_280, "a day of 5-second blocks");
    assert_eq!(ForkSchedule::default().lead(None), 86_400);
}

#[test]
fn a_genesis_lists_forks_this_release_knows_each_once() {
    assert_eq!(ForkSchedule { forks: vec![scheduled("test-fork", 0)], lead: None }.genesis_problem(), None);
    assert!(ForkSchedule { forks: vec![scheduled("from-the-future", 9)], lead: None }.genesis_problem().unwrap().contains("does not know"));
    assert!(ForkSchedule { forks: vec![scheduled("test-fork", 1), scheduled("test-fork", 9)], lead: None }.genesis_problem().unwrap().contains("twice"));
    assert!(ForkSchedule { forks: vec![scheduled("", 1)], lead: None }.genesis_problem().is_some());
}

#[test]
fn a_change_of_the_schedule_says_what_it_does() {
    let least = Version::new(0, 3, 0);

    assert_eq!(ForkChange::Schedule { name: "x".to_string(), at: ForkAt::Height(900), least }.describe(), "fork x from block 900, for chainv4 0.3.0 or newer");
    assert_eq!(ForkChange::Schedule { name: "x".to_string(), at: ForkAt::Epoch(9), least }.describe(), "fork x from epoch 9, for chainv4 0.3.0 or newer");
    assert_eq!(ForkChange::Cancel { name: "x".to_string() }.describe(), "cancel fork x");
    assert_eq!(ForkChange::Lead { blocks: 7 }.describe(), "forks scheduled 7 blocks ahead at least");
}

#[cfg(feature = "json")]
#[test]
fn the_apps_read_the_schedule_from_status_as_the_node_writes_it() {
    let schedule = ForkSchedule { forks: vec![scheduled("test-fork", 12), ScheduledFork { epoch: Some(3), ..scheduled("from-the-future", 30) }], lead: Some(4) };
    let status = serde_json::json!({ "forks": crate::params::json::forks_json(&schedule) });

    assert_eq!(crate::params::json::forks_from_status(&status), schedule);
    assert_eq!(crate::params::json::forks_from_status(&serde_json::json!({})), ForkSchedule::default(), "an older node's says none");
    assert!(crate::params::json::forks_from_status(&status).at(12).has(&TEST_FORK));
}

#[test]
fn every_fork_this_release_knows_has_a_bit_however_many_there_are() {
    assert!(FORK_WORDS * 64 >= FORKS.len(), "room for each");
    assert_eq!(Forks::default(), Forks::NONE);

    // Each fork alone in force is that fork and no other.
    for fork in FORKS {
        let alone = ForkSchedule { forks: vec![scheduled(fork.name, 1)], lead: None }.at(1);

        assert!(alone.has(fork), "{}", fork.name);
        assert_eq!(alone.names(), [fork.name]);
    }

    // All of them at once, oldest first.
    let all = ForkSchedule { forks: FORKS.iter().map(|fork| scheduled(fork.name, 1)).collect(), lead: None }.at(1);
    assert_eq!(all.names(), FORKS.iter().map(|fork| fork.name).collect::<Vec<_>>());
}
