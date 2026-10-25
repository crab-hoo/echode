use echode_draw::{ArrangeIndex, ArrangeView, Element, Role};
use echode_model::{Id, NoteDraft, Project, TICKS_PER_QUARTER, Tick};

fn scene(pitches: &[u8]) -> (Project, Id, Vec<Id>) {
    let (mut project, track) = Project::new().add_track("pitches");
    let mut ids = Vec::new();
    for (index, &pitch) in pitches.iter().enumerate() {
        let (next, id) = project
            .insert_note(
                track,
                NoteDraft {
                    pitch,
                    velocity: 100,
                    start: Tick::new(index as i64 * TICKS_PER_QUARTER),
                    length: Tick::new(TICKS_PER_QUARTER),
                },
            )
            .unwrap();
        project = next;
        ids.push(id);
    }
    (project, track, ids)
}

fn view(height: f64) -> ArrangeView {
    ArrangeView {
        origin_tick: Tick::new(0),
        width_ticks: 8 * TICKS_PER_QUARTER,
        origin_track: 0.0,
        height_tracks: 2.0,
        pixels_per_tick: 100.0 / TICKS_PER_QUARTER as f64,
        pixels_per_track: height,
    }
}

fn notes(index: &ArrangeIndex, project: &Project, view: ArrangeView) -> Vec<Element> {
    let mut notes = Vec::new();
    index.visit_visible(project, view, |element| {
        if element.id.role == Role::Note {
            notes.push(element);
        }
    });
    notes
}

fn near(actual: f32, expected: f64) {
    assert!(
        (f64::from(actual) - expected).abs() < 0.0001,
        "pitch geometry: got {actual}, expected {expected}"
    );
}

#[test]
fn contract_arrange_notes_use_content_pitch_range_and_keep_reserved_bands() {
    // Pitches and row heights exercise the supported UI zoom, not product limits.
    for pitches in [
        &[48, 60, 72][..],
        &[60, 61, 67],
        &[0],
        &[60],
        &[127],
        &[0, 127],
    ] {
        let (project, _, _) = scene(pitches);
        let index = ArrangeIndex::build(&project);
        let low = f64::from(*pitches.iter().min().unwrap());
        let high = f64::from(*pitches.iter().max().unwrap());
        let span = (high - low + 1.0).max(12.0);
        for row_height in [16.0, 64.0, 240.0] {
            let elements = notes(&index, &project, view(row_height));
            assert_eq!(elements.len(), pitches.len());
            let top = row_height * 0.14;
            let available = row_height * 0.72;
            let height = (available / span).max(1.0);
            for (element, &pitch) in elements.iter().zip(pitches) {
                let center =
                    top + available * (0.5 + ((low + high) / 2.0 - f64::from(pitch)) / span);
                let y = (center - height / 2.0).clamp(top, top + available - height);
                near(element.y, y);
                near(element.h, height);
                assert!(element.h >= 1.0);
                assert!(f64::from(element.y) >= top - 0.0001);
                assert!(f64::from(element.y + element.h) <= top + available + 0.0001);
            }
            if pitches.len() > 1 {
                assert!(elements.first().unwrap().y > elements.last().unwrap().y);
            }
        }
    }
}

#[test]
fn sub_point_rows_keep_notes_inside_the_reserved_bands() {
    let (project, _, _) = scene(&[0, 127]);
    let elements = notes(&ArrangeIndex::build(&project), &project, view(0.5));
    for element in elements {
        assert!(element.y.is_finite() && element.h > 0.0);
        assert!(element.y >= 0.07 - 0.0001);
        assert!(element.y + element.h <= 0.43 + 0.0001);
    }
}

#[test]
fn scrolling_and_trimmed_linked_windows_do_not_rescale_the_content() {
    let (project, track, ids) = scene(&[48, 60, 72]);
    let source = project.tracks()[0].clips[0].id;
    let (project, linked_track) = project.add_track("linked");
    let linked = Id::generate();
    let project = project
        .link_clip(source, linked_track, Tick::new(0), linked)
        .unwrap();
    let project = project
        .resize_clip(
            linked_track,
            linked,
            Tick::new(TICKS_PER_QUARTER),
            Tick::new(TICKS_PER_QUARTER),
        )
        .unwrap();
    let index = ArrangeIndex::build(&project);
    let whole = notes(&index, &project, view(64.0));
    let middle = whole
        .iter()
        .find(|e| e.id.track == track && e.id.note == Some(ids[1]))
        .unwrap();
    let linked_middle = whole
        .iter()
        .find(|e| e.id.track == linked_track && e.id.note == Some(ids[1]))
        .unwrap();
    near(linked_middle.y - 64.0, f64::from(middle.y));
    near(linked_middle.h, f64::from(middle.h));
    let partial = ArrangeView {
        origin_tick: Tick::new(TICKS_PER_QUARTER),
        width_ticks: TICKS_PER_QUARTER,
        ..view(64.0)
    };
    let scrolled = notes(&index, &project, partial);
    assert_eq!(scrolled.len(), 2);
    near(scrolled[0].y, f64::from(middle.y));
    near(scrolled[0].h, f64::from(middle.h));
    let before = notes(&index, &project, view(64.0));
    let edited = project.delete_note(track, ids[2]).unwrap();
    let updated = index.update(&edited);
    let after = notes(&updated, &edited, view(64.0));
    assert!(after.iter().find(|e| e.id.note == Some(ids[1])).unwrap().y < middle.y);
    assert_eq!(before, notes(&index, &project, view(64.0)));
}

#[test]
#[ignore = "release query measurement; run make spike-arrange-query"]
fn measure_product_arrange_query() {
    use echode_model::{
        ClipKind, Content, Lane, Meter, Note, Placement, Tempo, TempoMap, Track, midi7_to_velocity,
        test_mixer,
    };
    use std::{hint::black_box, time::Instant};

    // Match S1's population; these counts are measurement inputs, not DAW limits.
    let mut contents = Vec::new();
    let mut tracks = Vec::new();
    let mut ordering = Project::new();
    for _ in 0..1_000 {
        ordering = ordering.add_track("order").0;
    }
    for track in 0..1_000u128 {
        let content = Id::from_raw(1 + track * 1_000);
        let notes = (0..100u128)
            .map(|note| Note {
                id: Id::from_raw(4 + track * 1_000 + note),
                start: Tick::new(note as i64 * TICKS_PER_QUARTER),
                length: Tick::new(TICKS_PER_QUARTER),
                pitch: 48 + (note % 25) as u8,
                velocity: midi7_to_velocity(100),
                release: 0,
                articulation: None,
            })
            .collect();
        contents.push(Content {
            id: content,
            audio: None,
            takes: Vec::new(),
            notes,
            expressions: Vec::new(),
        });
        tracks.push(Track {
            id: Id::from_raw(2 + track * 1_000),
            name: format!("Track {track}"),
            parent: None,
            order: ordering.tracks()[track as usize].order.clone(),
            clips: vec![Placement {
                id: Id::from_raw(3 + track * 1_000),
                content,
                lane: Lane::Timeline,
                start: Tick::new(0),
                length: Tick::new(100 * TICKS_PER_QUARTER),
                offset: Tick::new(0),
                kind: ClipKind::Fixed,
                gain: 1.0,
                fade_in: 0,
                fade_out: 0,
            }],
            devices: vec![test_mixer(2 + track * 1_000)],
        });
    }
    let project = Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        contents,
        tracks,
    )
    .unwrap();
    let index = ArrangeIndex::build(&project);
    let mut out = Vec::new();
    index.visit_visible(
        &project,
        ArrangeView {
            width_ticks: 100 * TICKS_PER_QUARTER,
            height_tracks: 1_000.0,
            ..view(64.0)
        },
        |e| out.push(e),
    );
    for (name, view) in [
        (
            "full",
            ArrangeView {
                width_ticks: 100 * TICKS_PER_QUARTER,
                height_tracks: 1_000.0,
                ..view(64.0)
            },
        ),
        (
            "partial",
            ArrangeView {
                origin_tick: Tick::new(50 * TICKS_PER_QUARTER),
                width_ticks: 8 * TICKS_PER_QUARTER,
                origin_track: 500.25,
                height_tracks: 12.0,
                ..view(64.0)
            },
        ),
    ] {
        let mut elapsed = Vec::new();
        for round in 0..120 {
            let start = Instant::now();
            let result = black_box(index.visible(black_box(&project), view, black_box(&mut out)));
            let duration = start.elapsed();
            assert_eq!(result.1, 0);
            assert_eq!(result.0, if name == "full" { 102_000 } else { 130 });
            if round >= 20 {
                elapsed.push(duration);
            }
        }
        elapsed.sort();
        println!(
            "product arrange {name}: median {:?}, p99 {:?}, max {:?}",
            elapsed[50], elapsed[98], elapsed[99]
        );
    }
}
