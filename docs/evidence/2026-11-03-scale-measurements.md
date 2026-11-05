# 2026-11-03 規模の計測（コード全体の監査の証拠）

[コード全体の監査](../reviews/2026-11-03-full-code-audit.md) の FCA-01〜05・14 を支える計測である。

## 条件

- 対象: `5565b4d` と未コミット変更 23 件（別セッションの ADR 0238 の作業。計測した経路は変えていない）。
- 機器: Apple M4、24 GB、10 コア、macOS 27.0（26A428）。Rust 1.96.1、release。
- リポジトリの外の Cargo プロジェクトから、`echode-engine` と `echode-model` を path 依存で使った。リポジトリのファイルは計測に使っていない。
- 呼んだのは製品の公開 API だけである: `Project::try_assemble`、`Project::edit_note`、`Project::add_track`、
  `playback`、`PlaybackControl::{command, replace, service, status, reserved_events}`、`PlaybackAudio::render`。
- プラグイン・音声機器・ヘルパーは使っていない。48 kHz、128 フレーム（1 ブロック 2.67 ms）、120 bpm。
- 計測の一部は、別セッションの `verify.py` と同時に走った。通常ブロックの p50 は 236〜433 µs で揺れた。比は同じ行の値どうしで読む。
- 入力: 各トラックに固定窓のクリップを置き、短いノート（約 128 サンプルごと、長さは間隔の半分）と、必要なら全体を覆う長いノートを入れる。

## 再生成

```sh
mkdir -p <dir>/src   # リポジトリの外。<repo> は Echode の作業ツリー
# <dir>/Cargo.toml と <dir>/src/main.rs に下のプログラムを置く
CARGO_TARGET_DIR=<dir>/target cargo +1.96.1 build --release
<dir>/target/release/audit-bench model      # FCA-05
<dir>/target/release/audit-bench adopt      # FCA-02
<dir>/target/release/audit-bench position   # FCA-02（鳴らしたノート数との関係）
<dir>/target/release/audit-bench adoptloop  # FCA-04（制御側の replace の平均）
<dir>/target/release/audit-bench loop       # FCA-01
/usr/bin/time -l <dir>/target/release/audit-bench memory 4000       # FCA-03
/usr/bin/time -l <dir>/target/release/audit-bench projectonly 4000  # FCA-03 の比較
```

`Cargo.toml`:

```toml
[package]
name = "audit-bench"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
echode-engine = { path = "<repo>/crates/echode-engine" }
echode-model = { path = "<repo>/crates/echode-model" }

[workspace]
```

`src/main.rs`（274 行）:

```rust
//! Audit measurements. Not part of the Echode repository.
use std::time::{Duration, Instant};

use echode_engine::{PlaybackCommand, PlaybackOptions, playback};
use echode_model::{ClipKind, Content, Id, NoteDraft, NoteGeometry, Placement, Project, Tick, Track};

const RATE: u32 = 48_000;
const FRAMES: usize = 128;

struct Template {
    project: Project,
    clip: Placement,
    content: Content,
}

fn template(track_count: usize) -> Template {
    let mut project = Project::new();
    let mut tracks = Vec::new();
    for index in 0..track_count {
        let (next, id) = project.add_track(format!("t{index}"));
        project = next;
        tracks.push(id);
    }
    let (with_note, _) = project
        .insert_note(tracks[0], NoteDraft { pitch: 60, velocity: 100, start: Tick::new(0), length: Tick::new(1000) })
        .unwrap();
    let track = with_note.tracks().iter().find(|t| t.id == tracks[0]).unwrap();
    let clip = track.clips[0].clone();
    let content = (*with_note.contents()[0]).clone();
    Template { project, clip, content }
}

/// `clips` per track, each its own content with `notes` notes, fixed windows.
fn build(t: &Template, clips: usize, notes: usize, long_notes: usize, step: i64) -> Project {
    let mut contents = Vec::new();
    let mut tracks: Vec<Track> = Vec::new();
    let clip_len = (notes as i64 + 1) * step;
    for track in t.project.tracks() {
        let mut track = (**track).clone();
        for c in 0..clips {
            let mut content = t.content.clone();
            content.id = Id::generate();
            let mut list = Vec::new();
            for l in 0..long_notes {
                let mut note = t.content.notes[0].clone();
                note.id = Id::generate();
                note.pitch = 30 + l as u8;
                note.start = Tick::new(0);
                note.length = Tick::new(clip_len);
                list.push(note);
            }
            for n in 0..notes {
                let mut note = t.content.notes[0].clone();
                note.id = Id::generate();
                note.pitch = 60 + (n % 12) as u8;
                note.start = Tick::new(n as i64 * step);
                note.length = Tick::new(step / 2);
                list.push(note);
            }
            list.sort_by(|a, b| a.start.cmp(&b.start).then(a.pitch.cmp(&b.pitch)).then(a.id.cmp(&b.id)));
            content.notes = list;
            let mut clip = t.clip.clone();
            clip.id = Id::generate();
            clip.content = content.id;
            clip.kind = ClipKind::Fixed;
            clip.start = Tick::new(c as i64 * clip_len);
            clip.offset = Tick::new(0);
            clip.length = Tick::new(clip_len);
            track.clips.push(clip);
            contents.push(content);
        }
        tracks.push(track);
    }
    Project::try_assemble(t.project.meter(), t.project.tempo().clone(), contents, tracks).unwrap()
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

fn model_scale() {
    println!("== model: one edit and one load, clips each with their own content (8 notes)");
    println!("tracks clips/track | assemble(load) | edit_note | add_track");
    for (tracks, clips) in [(16, 16), (32, 32), (64, 64), (128, 64)] {
        let t = template(tracks);
        let project = build(&t, clips, 8, 0, 3_000);
        let (contents, tracks_v): (Vec<Content>, Vec<Track>) = (
            project.contents().iter().map(|c| (**c).clone()).collect(),
            project.tracks().iter().map(|t| (**t).clone()).collect(),
        );
        let (_, load) = time(|| Project::try_assemble(project.meter(), project.tempo().clone(), contents, tracks_v).unwrap());
        let track = &project.tracks()[0];
        let clip = &track.clips[0];
        let content = project.content(clip.content).unwrap();
        let note = &content.notes[0];
        let (_, edit) = time(|| {
            project
                .edit_note(track.id, clip.id, note.id, NoteGeometry { pitch: note.pitch + 1, start: note.start, length: note.length })
                .unwrap()
        });
        let (_, add) = time(|| project.add_track("x"));
        println!("{tracks:>6} {clips:>10} | {load:>14.2?} | {edit:>9.2?} | {add:>9.2?}");
    }
}

fn render_block(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) -> Duration {
    let start = Instant::now();
    audio.render(left, right);
    start.elapsed()
}

fn adoption(tracks: usize, notes: usize, long_notes: usize) {
    let t = template(tracks);
    // 3_763 ticks is about 128 samples at 120 bpm: one short note per block.
    let step = 3_763;
    let project = build(&t, 1, notes, long_notes, step);
    let (mut control, mut audio) = playback(&project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0f32; FRAMES];
    let mut right = vec![0.0f32; FRAMES];
    let mut normal = Vec::new();
    for _ in 0..notes.saturating_sub(4) {
        normal.push(render_block(&mut audio, &mut left, &mut right));
        control.service();
    }
    let track = &project.tracks()[0];
    let clip = &track.clips[0];
    let content = project.content(clip.content).unwrap();
    let note = content.notes.last().unwrap();
    let mut adopt = Vec::new();
    for round in 0..3 {
        let edited = project
            .edit_note(track.id, clip.id, note.id, NoteGeometry { pitch: note.pitch + 1 + round, start: note.start, length: note.length })
            .unwrap();
        control.replace(&edited).unwrap();
        adopt.push(render_block(&mut audio, &mut left, &mut right));
        control.service();
    }
    normal.sort();
    let p50 = normal[normal.len() / 2];
    let p99 = normal[normal.len() * 99 / 100];
    let max_adopt = adopt.iter().max().unwrap();
    println!(
        "tracks {tracks:>4} notes/track {notes:>5} held {long_notes:>2} | normal p50 {p50:>9.2?} p99 {p99:>9.2?} | adoption max {max_adopt:>9.2?} ({:.1}x block budget 2.67ms)",
        max_adopt.as_secs_f64() / (FRAMES as f64 / RATE as f64)
    );
}

fn adoption_loop(tracks: usize, notes: usize) {
    let t = template(tracks);
    let step = 3_763;
    let project = build(&t, 1, notes, 0, step);
    let (mut control, mut audio) = playback(&project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0f32; FRAMES];
    let mut right = vec![0.0f32; FRAMES];
    for _ in 0..notes.saturating_sub(400) {
        render_block(&mut audio, &mut left, &mut right);
        control.service();
    }
    eprintln!("pid {} ready", std::process::id());
    let track = &project.tracks()[0];
    let clip = &track.clips[0];
    let content = project.content(clip.content).unwrap();
    let note = content.notes.last().unwrap();
    let mut audio_total = Duration::ZERO;
    let mut control_total = Duration::ZERO;
    for round in 0..300u32 {
        let edited = project
            .edit_note(track.id, clip.id, note.id, NoteGeometry { pitch: 61 + (round % 2) as u8, start: note.start, length: note.length })
            .unwrap();
        let start = Instant::now();
        control.replace(&edited).unwrap();
        control_total += start.elapsed();
        audio_total += render_block(&mut audio, &mut left, &mut right);
        control.service();
    }
    println!("300 adoptions: control replace avg {:?}, audio adoption block avg {:?}", control_total / 300, audio_total / 300);
}

fn adoption_at(tracks: usize, notes: usize, played: usize) {
    let t = template(tracks);
    let step = 3_763;
    let project = build(&t, 1, notes, 0, step);
    let (mut control, mut audio) = playback(&project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0f32; FRAMES];
    let mut right = vec![0.0f32; FRAMES];
    for _ in 0..played {
        render_block(&mut audio, &mut left, &mut right);
        control.service();
    }
    let track = &project.tracks()[0];
    let clip = &track.clips[0];
    let content = project.content(clip.content).unwrap();
    let note = content.notes.last().unwrap();
    let mut adopt = Vec::new();
    for round in 0..20u32 {
        let edited = project
            .edit_note(track.id, clip.id, note.id, NoteGeometry { pitch: 61 + (round % 2) as u8, start: note.start, length: note.length })
            .unwrap();
        control.replace(&edited).unwrap();
        adopt.push(render_block(&mut audio, &mut left, &mut right));
        control.service();
    }
    adopt.sort();
    println!("tracks {tracks} notes {notes} played blocks {played:>5} | adoption p50 {:>9.2?} max {:>9.2?}", adopt[10], adopt[19]);
}

fn memory(tracks: usize, notes: usize) {
    let t = template(tracks);
    let project = build(&t, 1, notes, 0, 3_763);
    let (control, audio) = playback(&project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    println!("built playback for {tracks} x {notes} notes; reserved events {}", control.reserved_events());
    std::hint::black_box((&control, &audio));
}

fn loop_wrap(tracks: usize, notes: usize) {
    let t = template(tracks);
    let step = 3_763;
    let project = build(&t, 1, notes, 0, step);
    let (mut control, mut audio) = playback(&project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    // A 32-block loop near the end of the notes.
    let end = (notes as i64 - 8) * FRAMES as i64;
    let start = end - 32 * FRAMES as i64 - 37;
    control.command(PlaybackCommand::Locate(start)).unwrap();
    control.command(PlaybackCommand::SetLoop { start, end }).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0f32; FRAMES];
    let mut right = vec![0.0f32; FRAMES];
    let mut wraps = Vec::new();
    let mut others = Vec::new();
    let mut last = control.status().sample;
    for _ in 0..400 {
        let took = render_block(&mut audio, &mut left, &mut right);
        control.service();
        let now = control.status().sample;
        if now < last { wraps.push(took) } else { others.push(took) }
        last = now;
    }
    others.sort();
    wraps.sort();
    println!(
        "tracks {tracks:>4} notes/track {notes:>6} | other blocks p50 {:>9.2?} | wrap blocks n={} p50 {:>9.2?} max {:>9.2?}",
        others[others.len() / 2], wraps.len(), wraps.get(wraps.len() / 2).copied().unwrap_or_default(), wraps.last().copied().unwrap_or_default()
    );
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_default();
    if which.is_empty() || which == "model" { model_scale(); }
    if which.is_empty() || which == "adopt" {
        println!("== audio thread: block that adopts a replaced plan while playing (no plugins)");
        for (tracks, notes, held) in [(16, 2000, 8), (64, 2000, 8), (256, 2000, 8), (256, 4000, 8)] {
            adoption(tracks, notes, held);
        }
    }
    if which == "projectonly" { let n: usize = std::env::args().nth(2).unwrap().parse().unwrap(); let t = template(256); let p = build(&t, 1, n, 0, 3_763); std::hint::black_box(&p); }
    if which == "memory" { let n: usize = std::env::args().nth(2).unwrap().parse().unwrap(); memory(256, n); }
    if which == "position" { for played in [100, 1000, 2000, 3500] { adoption_at(256, 4000, played); } }
    if which == "adoptloop" { adoption_loop(256, 4000); }
    if which == "held" {
        println!("== adoption vs held notes (256 tracks, 4000 notes)");
        for held in [0, 1, 8, 32] { adoption(256, 4000, held); }
    }
    if which.is_empty() || which == "loop" {
        println!("== audio thread: loop wrap with notes before the loop (no plugins)");
        for (tracks, notes) in [(16, 4000), (64, 4000), (256, 4000), (256, 16000)] {
            loop_wrap(tracks, notes);
        }
    }
}
```

## 出力の抜粋

### FCA-01: ループの折り返し（2 回目）

```text
tracks   16 notes/track   4000 | other blocks p50   12.12µs | wrap blocks n=12 p50   68.38µs max  300.83µs
tracks   64 notes/track   4000 | other blocks p50   54.46µs | wrap blocks n=12 p50  320.29µs max    1.10ms
tracks  256 notes/track   4000 | other blocks p50  237.04µs | wrap blocks n=12 p50    2.68ms max    3.78ms
tracks  256 notes/track  16000 | other blocks p50  237.83µs | wrap blocks n=12 p50    8.09ms max   51.21ms
```

1 回目の 256 × 16,000 は p50 7.10 ms、最大 11.82 ms だった。最大値は同時に走った別の処理の影響を含み得る。

### FCA-02: 再生中の編集を採用するブロック

```text
tracks   16 notes/track  2000 held  8 | normal p50   12.08µs p99   14.46µs | adoption max  632.46µs (0.2x block budget 2.67ms)
tracks   64 notes/track  2000 held  8 | normal p50   54.17µs p99   64.17µs | adoption max    2.19ms (0.8x block budget 2.67ms)
tracks  256 notes/track  2000 held  8 | normal p50  238.75µs p99  253.29µs | adoption max    6.78ms (2.5x block budget 2.67ms)
tracks  256 notes/track  4000 held  8 | normal p50  240.38µs p99  253.58µs | adoption max   10.22ms (3.8x block budget 2.67ms)
```

```text
tracks 256 notes 4000 played blocks   100 | adoption p50    1.53ms max    2.36ms
tracks 256 notes 4000 played blocks  1000 | adoption p50    1.83ms max    3.36ms
tracks 256 notes 4000 played blocks  2000 | adoption p50    2.20ms max    4.68ms
tracks 256 notes 4000 played blocks  3500 | adoption p50    2.83ms max    5.66ms
```

`adopt` の各行は 3 回の採用の最大で、最初の採用はキャッシュが冷えている。`position` は 20 回の採用の p50 と最大である。
保持音の数（0・1・8・32）を変えた回では、0 でも最初の採用が 13.4 ms だった。主な費用は保持音の探索ではなく、鳴らしたノートの列の走査と冷えたメモリである。

### FCA-03・04: メモリと制御側の作り直し

```text
built playback for 256 x 1000 notes; reserved events 1024001
           301760512  maximum resident set size
built playback for 256 x 4000 notes; reserved events 4096001
          1184710656  maximum resident set size
(projectonly 4000)
           199245824  maximum resident set size
300 adoptions: control replace avg 278.083421ms, audio adoption block avg 2.977852ms
```

`adoptloop` の間に `sample` で 8 秒取った標本では、`PlaybackControl::replace` → `Prepared::new` → `build_plan` が大半を占めた。
`build_plan` の中では `PlaybackIds` の `HashMap`（`BuildHasher::hash_one`）と `TempoIndex::tick_to_sample` の i128 除算が目立った。

### FCA-05: モデルの編集と読込みの検証（2 回目）

```text
tracks clips/track | assemble(load) | edit_note | add_track
    16         16 |       500.83µs |   57.42µs |   49.88µs
    32         32 |         4.47ms |  664.21µs |  937.46µs
    64         64 |        39.82ms |    8.32ms |  381.21µs
   128         64 |       166.96ms |   35.96ms |    1.03ms
```

### FCA-14: ジャーナルの 1 行の追記と同期

Python で、`O_APPEND` の 1 行（約 100 バイト）の書込みと同期を 200 回ずつ測った。ファイルは scratchpad の APFS 上である。

```text
none     p50=0.003ms p90=0.005ms p99=0.012ms max=0.048ms
fsync    p50=0.030ms p90=0.037ms p99=0.067ms max=0.081ms
barrier  p50=0.313ms p90=2.600ms p99=3.670ms max=4.105ms
full     p50=3.996ms p90=4.156ms p99=4.239ms max=44.610ms
```

`full` は `fcntl(F_FULLFSYNC)`、`barrier` は `fcntl(F_BARRIERFSYNC)` である。Rust の `File::sync_all` は macOS で `F_FULLFSYNC` を使う。

## 判定の範囲

- プラグインなしの値である。ヘルパーの投入・回収・再マップの時間は含まない。
- 128 フレームの期限で判定した。64 フレーム以下では期限が短い。
- 製品のアプリと音声機器の経路（HAL の I/O スレッド、Workgroup）は通していない。
