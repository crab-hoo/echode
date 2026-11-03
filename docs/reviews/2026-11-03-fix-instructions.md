# 2026-11-03 コード全体の監査の実行指示

この指示書は、[コード全体の監査](2026-11-03-full-code-audit.md)（FCA-01〜20）を直す実行役への作業契約である。
指摘の根拠と計測は監査と [規模の計測](../evidence/2026-11-03-scale-measurements.md) にある。
ここでは順番、触る場所、守る境界、完了条件だけを決める。

## 作業を始める前に

1. AGENTS.md を読み、`make agent-context` を実行する。担当する作業単位の監査の節と関連 ADR だけを読む。
2. 環境の状態を確かめる。
   - Claude Code は編集ツールが検査のロックを待ち、終了時に `make verify` の判定を受ける（`.claude/settings.json`）。
   - 作業ツリーは他のセッションと共有され得る。他の作業の失敗で `make verify` が通らない時は、その作業を直さず、待つかユーザーに相談する（XCA-07）。
   - `make spike-scale` は最大で約 2 GB のメモリを使い、約 25 秒かかる。実行前に `df -h` と他の計測の有無を見る。
3. 修正の前に再現が失敗することを確かめる。
   - 4 件は `make known-bugs` で走る（下の表）。1 件だけなら `cargo test -q -p <crate> --test <file> -- --ignored --exact <name>`。
   - 改善前の値は `make spike-scale` で取り、`target/spike-scale.json` を ADR に写す。2026-11-03 の値は下にある。
4. 意図した red の確認では、`--exact` のテスト名を正しく書く。一致しないと 0 件の実行が成功になる。

| 再現・計測 | 対象 | 場所 |
|---|---|---|
| `audit_fca01_a_loop_wrap_releases_only_sounding_notes` | FCA-01 | `crates/echode-realtime/tests/audit.rs` |
| `audit_fca03_event_slots_follow_one_block_not_the_whole_track` | FCA-03 | `crates/echode-engine/tests/audit_scale_and_mix.rs` |
| `audit_fca06_a_folder_mute_and_solo_reach_its_children` | FCA-06 | 同上 |
| `audit_fca16_a_float_wav_has_an_extended_format_and_a_fact_chunk` | FCA-16 | `crates/echode-cli/tests/audit_wav.rs` |
| `measure_playback_scale`（`make spike-scale`） | FCA-01・02・04・05 | `crates/echode-engine/tests/scale.rs` |

FCA-01 の再現は、折り返しで鳴っていないノートへ送るノートオフという見える症状だけを固定する。走査の量は `make spike-scale` で見る。
`spike-scale` はどの変更でも選ばれない明示の計測である。回帰は W1 で足す時間に依らないテストで固定する。

2026-11-03 の `make spike-scale`（Apple M4、release、プラグインなし、128 フレーム、期限 2.67 ms）:

| 場合 | 条件 | 通常ブロック p50 | 対象のブロック p50 | 最大 | その他 |
|---|---|---:|---:|---:|---|
| 折り返し | 64 トラック × 1,000 ノート | 53.9 µs | 146.6 µs | 289.1 µs | |
| 折り返し | 64 × 4,000 | 54.1 µs | 557.4 µs | 1,338 µs | |
| 折り返し | 64 × 16,000 | 54.4 µs | 2,512 µs | 3,859 µs | |
| 折り返し | 256 × 4,000 | 237.3 µs | 2,735 µs | 3,879 µs | |
| 再生中の編集 | 256 × 4,000、100 ブロック後 | 240.9 µs | 1,278 µs | 2,139 µs | replace p50 268.6 ms、予約 4,096,001 件 |
| 再生中の編集 | 256 × 4,000、3,500 ブロック後 | 240.2 µs | 2,803 µs | 7,007 µs | replace p50 273.5 ms |
| モデル | 64 × 64 クリップ | | | | 読込みの検証 38.8 ms、ノートの編集 8.0 ms |
| モデル | 128 × 64 クリップ | | | | 読込みの検証 165.1 ms、ノートの編集 35.8 ms |

## W1: オーディオスレッドの楽譜に比例する処理（FCA-01・02）— P1、最初に行う

2026-11-03: 実装は [ADR 0239](../decisions/0239-live-note-cursor-work.md) で対応した。実機の聴感は UC-47 に残す。

- 場所: `crates/echode-realtime/src/lib.rs` の `TrackCursor`（`hold`・`clear`・`place`）、`Cursor` の `release_covering`・`release_replaced`・`keep_live_keys`、`covers`、`live_key_of`。
  `crates/echode-engine/src/playback.rs` の `Prepared::carry`・`queue_replaced`・`queue_all_offs`・`place`。
- 契約:
  - 1 ブロックの処理は、そのブロックの端の数と鳴っているノートの数とトラック数に比例する。楽譜の全ノート数や、位置合わせ以降に鳴らしたノート数に比例しない。
  - 折り返しは鳴っているノートだけを止める（`Cursor::play` の説明のとおり）。ループ前からのノートを折り返しで追いかけない今の方針は変えない。
  - 計画の差替えで、鳴り続けるノートのノートオフは、ノートオンの時の鍵と番号を使う（`a_replaced_note_offs_with_the_key_it_started_on` ほか既存の回帰）。
  - オーディオスレッドで確保・ロックをしない。索引（ノートのモデル ID から位置、トラックの ID から位置）は制御スレッドで計画と一緒に作る。
- 回帰: 時間に依らない回帰を足す。テスト用の計数で 1 ブロックに訪ねたノートを数え、ノート数を 16 倍にしても増えないことを、折り返し・差替え・位置移動の 3 つで固定する。
  計数を製品のコードに置く時は、release の処理と確保を変えない形にする。必須契約にするかは [検査手順](../verification.md) の基準で決める。
- 完了条件:
  - `audit_fca01` の `#[ignore]` を外して通る。
  - `make spike-scale` で、64 トラックの折り返しの p50 が 1,000〜16,000 ノートでほぼ一定になる。256 × 4,000 の編集のブロックが 100 ブロック後と 3,500 ブロック後でほぼ同じになる。前後の値を ADR に書く。
  - `echode-realtime` と `echode-engine` の全テスト、`block_size.rs`、`rtsan`、関係する必須契約が通る。
- 境界: ヘルパーとの通信、ミックス、保存形式を変えない。プラグインのある経路の値（`spike-plugin-tracks`）が悪化しないことを確かめる。

## W2: 規模（FCA-03・04・05・10）— P2

2026-11-03: FCA-03 は [ADR 0240](../decisions/0240-block-local-event-capacity.md) で対応した。
FCA-10 は [ADR 0241](../decisions/0241-unscaled-placement-pcm.md) で対応した。
FCA-05 の先行分は [ADR 0242](../decisions/0242-model-content-lookup-and-references.md) で対応した。ID の探索と参照の差分更新、FCA-04 は残す。

- FCA-03: `playback.rs` の `track_event_demand`、`plugins.rs` の `fit_stage`・`reconcile`・`can_reconcile`。
  1 ブロックの容量は、整列済みの端を幅 `frames` の窓で数えた最大（ループの周回を含む）とし、追いかけは重なりの最大にする。
  容量は 2 のべき乗へ切り上げ、増える時だけ再マップする。`audit_fca03` を通し、`plugin_event_caps` で共有領域の容量が全ノート数に比例しない回帰を足す。
  容量を超えるイベントを黙って落とさない既存の契約（`dropped_events` で失敗にする）を保つ。
- FCA-04: `PlaybackControl::replace`・`prepare_plan`、`Prepared::new`、`plan::build_plan`、`PlaybackIds`。
  `Arc<Track>`・`Arc<Content>` の同一性で、変わらないトラックの計画・カーソル・バッファを使い回す。描画の `ArrangeIndex::update` が手本になる。
  `spike-scale` の `replace_p50_ms` を下げ、前後の値を ADR に書く。再生中の編集でノートの番号が変わらない回帰を保つ。
- FCA-05: `echode-model` の `model.rs`（`content`・`id_in_use`・`try_assemble`・`overlaps_others`）と `placement.rs`（`drop_unreferenced`）。
  内容の ID 順を不変条件にして二分探索する。未参照の除去は変えた配置だけで判定する。重なりは開始順の隣だけを比べる。
  `check_invariants` と `crates/echode-history/tests/properties.rs` を通す。`spike-scale` の `model_edit` が、クリップ 2 倍で 2 倍程度に収まる。保存形式は変えない。
- FCA-10: `crates/echode-audio-io/src/clips.rs` の `WindowKey` からゲインを外し、ゲインとモノの昇格を `playback.rs` の `add_stored_clips` で係数として掛ける。
  音を作る処理が変わるので、`crates/echode-engine/tests/block_size.rs` にブロック長を変えて比べるテストを足す（AGENTS）。
  基準音声が変わる場合は `make bless` をせず、差の理由と一緒にユーザーに相談する。

## W3: ミックスとプラグインの意味（FCA-06・07・08）— P2

2026-11-03: FCA-06 のミュート・ソロの子孫への反映と入れ子の回帰を [ADR 0244](../decisions/0244-folder-mute-and-solo-gates.md) で接続した。
音量・パンの意味はユーザーの判断を待ち、バス化とともに残す。
FCA-08 のトラックごとの 5 ms の減衰・復帰は [ADR 0245](../decisions/0245-plugin-deadline-fades.md) で接続した。実機の聴感は UC-50 に残す。

- FCA-06: エンジンの `Prepared::new` のミュートとソロの計算に、モデルの親子（`Track::parent`）を使う。
  既定は、フォルダのミュートで子孫を無音にし、フォルダのソロで子孫をソロとして扱う。`audit_fca06` を通し、入れ子のフォルダの回帰も足す。
  フォルダの音量・パンを子へ掛けるか、バス化（P2-03）まで操作子を無効にするかは、ユーザーに確認してから ADR に書く。
  ADR 0116 の未決事項と [現在の制約](../status.md) を更新する。
- FCA-07: `crates/echode-plugin-host/src/threads.rs` で、起動と再起動の要求の後に `clap.latency` を主スレッドで読み、`PluginRestarted` に実際の値を載せる。
  親（`crates/echode-sandbox/src/session.rs`）は値を持ち、デバイスビューへ出す。ホスト側の `clap.latency`・`clap.log`・`clap.thread-check` を足す。
  `echode-test-plugin` に遅延を宣言する故障を足し、`hosted_playback` で報告された値を確かめる。
  遅延の補正（PDC）は P2-03 で行い、この作業に入れない。
- FCA-08: `crates/echode-engine/src/plugins.rs` の `apply` の `Run::Missed`。取りこぼしの始まりと戻りを数 ms でつなぎ、段差を出さない。
  方式（直前の値からの減衰、戻りのフェードの長さ）は ADR で決め、[設計書の 9.4](../architecture/plugins-and-devices.md) を実装に合わせる。
  `echode-test-plugin` の停止の故障で取りこぼしを起こし、前後の標本の差に上限がある回帰を `hosted_playback` に足す。

## W4: 待ちの期限（FCA-09）— P2

- 場所: `crates/echode-audio-io/src/stream.rs` の `fill_inner`（期限）、`crates/echode-engine/src/plugins.rs` の `collect_deadline`、`audition.rs` の IOProc。
- 実行役は読解と、機器なしの計測までを行う。2 つの IOProc の実機の挙動は UC-16 でユーザーが確かめる。
- 変更案と根拠を ADR の草案にし、UC-16 の結果が出るまで既定の期限を変えない。

## W5: 整理（FCA-11〜20）— P3

- FCA-11: `crates/echode-engine/src/handoff.rs` とそのテストを消し、`lib.rs` の公開をやめる。リングの命令を別の列挙型にし、`PreviewFader` の `abort` と届かない分岐を型から消す。
- FCA-12: `crates/echode-sandbox/src/sync.rs` の `os_sync_*` と `pthread_set_qos_class_self_np` を `libc` へ置き換える。
  `scripts/check-ffi.sh` を関数名の単位にする時は、検査のスクリプトの変更として全自動検査が選ばれる。
- FCA-13: 時間の換算を依存のない 1 つの crate にまとめる。丸めの結果が変わらないことを、既存の時間のテストと `block_size.rs` で確かめる。
- FCA-14: `repair_journal` は末尾だけを読む。続く編集のまとめ書きは確定の意味に触れるので、ADR とユーザーの確認を先にする。
- FCA-15: Sine Pad の DSP を Sine Pad の crate か小さな DSP の crate へ移し、`TestSynth` の名前を直す。Sine Pad の状態と音は変えない。
- FCA-16: `crates/echode-cli/src/lib.rs` の `wav_header` を 18 バイトの `fmt` と `fact` にし、`audit_fca16` を通す。
  `crates/echode-cli/tests/render.rs` などの 44 バイトのヘッダの期待値も直す。RF64 と整数形式は別の作業単位にする。
  2026-11-03: ヘッダと回帰は [ADR 0243](../decisions/0243-float-wav-metadata.md) で対応した。RF64・整数形式は未着手。
- FCA-17〜20: 監査の修正方針に従う。FCA-18 の保管の形式、FCA-19 のメモリの予算、FCA-20 の走査の期限は、ADR とユーザーの確認を先にする。

## 実行役が決めないこと

次はユーザーの判断であり、指示があるまで行わない。

- 保存形式・スキーマ・ジャーナルの確定の意味の変更（FCA-14 のまとめ書き、FCA-18）。
- フォルダの音量・パンの意味（FCA-06）と、待ちの期限の既定値（FCA-09）。
- 基準音声の再生成（`make bless`）、`make verify-full`、実機・聴感・第三者プラグインの確認、リモートへのプッシュ。
- `target/` のキャッシュの削除。

## 共通の完了条件

- 1 つの作業単位を 1 つのコミットにする。他のセッションの変更を含めない。
- 直した項目の再現の `#[ignore]` を外し、[監査](2026-11-03-full-code-audit.md) の状態の列と `docs/status.md` の制約を更新する。
- 設計を選んだ箇所は ADR に残す。`spike-scale` で測った作業単位は、改善前後の値を ADR に書く。
- 最後に `make verify` を通す。実機・聴感を見送る時は、検査名と理由を残す。
- W1 が終わった時点で、`make verify-full` を行うかをユーザーに相談する。
