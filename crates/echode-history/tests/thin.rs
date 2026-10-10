use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_history::{
    DAY, HOUR, RECENT, git_store_size, kept_indexes, pack_and_thin, seed_autosaves, tip_text,
    write_bundle,
};

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("echode-thin-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

#[test]
fn packing_keeps_the_tip_and_hourly_thinning_drops_the_middle() {
    let directory = scratch();
    let origin = 1_700_000_000;
    let points = seed_autosaves(&directory, 12, 10, origin).unwrap();
    let before = git_store_size(&directory.join(".git"));
    let times: Vec<_> = points.iter().map(|point| point.time).collect();
    let now = *times.last().unwrap();
    let kept = kept_indexes(&times, now);
    assert_eq!(kept.len(), points.len());
    let tip = pack_and_thin(&gix::open(&directory).unwrap(), &points, &kept).unwrap();
    let packed = git_store_size(&directory.join(".git"));
    assert!(packed.files < before.files);
    assert_eq!(
        tip_text(&gix::open(&directory).unwrap()).unwrap(),
        "save 11"
    );
    let later = now + RECENT + HOUR;
    let thinned = kept_indexes(&times, later);
    assert!(thinned.len() < points.len());
    let bundle = directory.join("history.bundle");
    let bundle_bytes = write_bundle(&gix::open(&directory).unwrap(), tip, &bundle).unwrap();
    let header = std::fs::read(&bundle).unwrap();
    assert!(header.starts_with(b"# v2 git bundle\n"));
    assert!(bundle_bytes > 16);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
#[ignore = "HIS-5: a day of autosave, then pack and thin"]
fn a_day_of_autosave_packs_and_thins() {
    // 10 seconds for 8 hours is the audit's example, not a product limit.
    const SAVES: usize = 8 * 60 * 60 / 10;
    let directory = scratch();
    let origin = 1_700_000_000;
    let points = seed_autosaves(&directory, SAVES, 10, origin).unwrap();
    let before = git_store_size(&directory.join(".git"));
    let times: Vec<_> = points.iter().map(|point| point.time).collect();
    let now = *times.last().unwrap();
    let kept = kept_indexes(&times, now);
    assert_eq!(kept.len(), SAVES);
    pack_and_thin(&gix::open(&directory).unwrap(), &points, &kept).unwrap();
    let packed = git_store_size(&directory.join(".git"));
    let _ = std::fs::remove_dir_all(&directory);

    let thinned_dir = scratch();
    let points = seed_autosaves(&thinned_dir, SAVES, 10, origin).unwrap();
    let times: Vec<_> = points.iter().map(|point| point.time).collect();
    let now = *times.last().unwrap();
    let later = now + RECENT + DAY;
    let thinned_indexes = kept_indexes(&times, later);
    let tip = pack_and_thin(&gix::open(&thinned_dir).unwrap(), &points, &thinned_indexes).unwrap();
    let thinned = git_store_size(&thinned_dir.join(".git"));
    assert_eq!(
        tip_text(&gix::open(&thinned_dir).unwrap()).unwrap(),
        format!("save {}", SAVES - 1)
    );
    println!(
        "HIS-5 saves={SAVES} loose_files={} loose_bytes={} packed_files={} packed_bytes={} kept_recent={} thinned_commits={} thinned_files={} thinned_bytes={} tip={tip}",
        before.files,
        before.bytes,
        packed.files,
        packed.bytes,
        kept.len(),
        thinned_indexes.len(),
        thinned.files,
        thinned.bytes,
    );
    let _ = std::fs::remove_dir_all(&thinned_dir);
}
