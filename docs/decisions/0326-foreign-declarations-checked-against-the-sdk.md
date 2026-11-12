# 0326. 残る外部宣言を 1 か所に集め、SDK のヘッダと照合する

- 日付: 2026-11-12
- 状態: 採用
- 関連: [ENG-15](../reviews/2026-10-08-audit.md#eng-15-手書きの-ffi-を減らし残すものは-sdk-のヘッダと突き合わせる)、[統合表の M9](../reviews/2026-11-08-open-items-register.md#2-保守性)、
  [ADR 0265](0265-symbol-scoped-foreign-declarations.md)（シンボル単位の許可）、[ADR 0278](0278-audit-backlog-order.md)（順番）、
  [ADR 0276](0276-priority-order-and-delegated-decisions.md)（優先順位）

## 背景

開始時の HEAD は `6bacc3b`。

ENG-15 の完了の条件は「OS のフレームワークの宣言が手書きでなくなり、残った手書きの宣言には、SDK と突き合わせるテストがある」である。
ADR 0265 の後に残った手書きは 14 シンボルで、どれも SDK と照合していなかった。

- `echode-sandbox` の `sync.rs`: Workgroup の 4 関数、`os_release`、Mach の `mach_port_deallocate`・`mach_msg`、bootstrap の 2 関数と `bootstrap_port`。
  隣の `workgroup.rs` は、Mach のメッセージの配置（40 バイト）と 6 個の定数も手で書いていた。
- `echode-engine` の `graph.rs`（spike）: Workgroup の 3 関数と、join の token の構造体の 2 つ目の定義。
- `echode-engine` の `device.rs`: `os_release`。

ほかに、`sync.rs` と `wait.rs` が errno と `os_sync` のフラグの値を手で書いていた（libc にある）。
`device.rs`・`echode-audio-io`・`workgroup.rs` は、libc が「mach2 を使う」として非推奨にした Mach の時計と `mach_task_self` を、`allow(deprecated)` で使っていた。

## 調べたこと

| 候補 | 内容 | 結果 |
|---|---|---|
| libc 0.2.190（最新の安定版） | `mach_task_self_` のみ。`mach_msg`・bootstrap・`mach_port_deallocate` はない | 使えない |
| mach2 0.7.0 | `mach_msg` とメッセージの型・定数、bootstrap、`mach_port_*`、`mach_task_self`、Mach の時計 | 使う |
| dispatch2 0.3.1・objc2 の系統 0.3.2 | `os_workgroup_*`・`os_release` はない | 使えない |

mach2 は BSD-2-Clause OR MIT OR Apache-2.0、依存とビルドスクリプトがなく、edition 2024（ツールチェインは 1.96.1）である。
製品とテストが使う mach2 の 9 個の関数・2 個の静的変数（`bootstrap_port`・`mach_task_self_`）・11 個の定数と、メッセージの 3 つの構造体と `mach_timebase_info` の大きさと位置を、作業中に C で一度だけ SDK と照合し、全て一致した。
メッセージの記述子のビットフィールド（`pad2:16`・`disposition:8`・`type:8`）が、mach2 の `u16`・`u8`・`u8` と同じバイトに入ることも確かめた。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | 14 シンボルを手書きのまま 1 か所に集め、全てに SDK の照合を付ける | 採らない |
| 2 | mach2 にある Mach・bootstrap を置き換え、残る Workgroup と `os_release` だけを 1 か所に集めて照合する | 採る |

- 案 1 は依存を増やさないが、Mach のメッセージの配置と定数も手で持ち続ける。照合は今の C の型で書ける範囲に限られ、`mach_msg_header_t` のような名前のない構造体は照合の対象にできない。
- 案 2 は ENG-15 の方針（既にある束縛を使い、どこにもないものだけを手で書く）と、libc の非推奨の案内に合う。
  手書きは 6 シンボルと構造体 3 つになり、Mach の束縛の出所が 1 つ（mach2）になる。依存は 1 つ増える。

## 決定

- 手書きの宣言は `crates/echode-sandbox/src/os_workgroup.rs` だけに置く。`os_workgroup_*` の 5 関数、`os_release`、SDK と同じ名前の構造体 3 つである。
  エンジンは `echode_sandbox::os_workgroup` を使い、join の token の 2 つ目の定義と、同じ規則の `apply_time_constraint` の写しを除いた。
  Workgroup のポインターは `*mut os_workgroup_s` にし、CoreAudio のプロパティの値を受ける所だけで型を変える。
- Mach のポート・メッセージ・bootstrap・時計は mach2 0.7.0 を使う。libc が非推奨にした Mach の宣言と、その `allow(deprecated)` は残さない。
  errno と `os_sync` のフラグは libc の定数を使う。
- `scripts/ffi-allowlist.txt` の各行は、パス・Rust の名前・宣言する SDK のヘッダの 3 つにする。
- `check-ffi` は、許可一覧と宣言の照合（ADR 0265）が通った後、許可した宣言を C に訳してヘッダごとに `xcrun clang -fsyntax-only` で調べる。
  - 関数は `__typeof__(&名前)` と、静的変数は `__typeof__(名前)` と、`__builtin_types_compatible_p` で同じ C の型かを調べる。
    `u32` と `unsigned int` のような同じ型の別名は通り、符号・`const`・指す先の構造体の違いは拒否する。
  - 宣言が指す構造体は `#[repr(C)]` をちょうど持ち、SDK の同じタグの構造体と大きさ・揃え・各フィールドの位置と型が一致する。
    フィールドが `_` で始まるものだけの構造体は不透明として、タグの一致だけを見る。
  - 非推奨の印と、`scripts/build-platform.env` の配置対象（27.0）より新しい API は、clang の診断で拒否する。
  - 訳せない Rust の型（総称、関数ポインター、可変長引数、配列の値渡し）は理由を示して拒否し、黙って通さない。

## 回帰と測定

- `ForeignDeclarationsMatchTheSdk`（`scripts/tests/test_audit_reproductions.py`）は、検査の入口を小さな Rust の木で動かす。
  引数の型・指す先の `const`・戻り値の符号・静的変数の型・構造体の大きさ・フィールドの型・`#[repr(C)]` の欠落・訳せない型・非推奨（`sem_init`）・
  配置対象より新しい API（配置対象 10.15 での `os_workgroup_join`）・ヘッダにない名前を拒否し、一致する宣言を通すことを確かめる。
  修正前の検査では 11 件の拒否のどれも報告されず、3 列の許可一覧も受け付けなかった。
- 既存の `ForeignDeclarations` は 3 列の許可一覧へ移した。生の識別子とコールバックの型の名前の読み取りは、宣言の走査を直接呼んで確かめる。
- `check-ffi` 全体は 0.72 秒（3 回とも同じ）で、2 回のコンパイルを含む。

## 作業中に見つけた不具合

ヘルパーは `bootstrap_check_in` で得た受信権を `mach_port_deallocate` で解放していた。
受信権には効かず `KERN_INVALID_RIGHT`（17）が返り、受信権と launchd の名前の登録がヘルパーの寿命まで残っていた（C の試験で確認）。
受信の後に `mach_port_mod_refs` で受信権の参照を 1 つ減らす。
`workgroup_tests.rs` の `a_received_workgroup_port_releases_the_checked_in_name` は、受け渡しの後に名前が登録から消えることを確かめ、修正前は 2 秒後も `bootstrap_look_up` が 0 を返して失敗した。

## 残る範囲

- mach2 の宣言は、作業中の一度の照合だけで、継続の検査はない。mach2 の版を上げる時は、同じ照合をやり直す。
- 照合はタグを持つ構造体に限る。`typedef` だけの名前のない構造体を手で宣言する必要が出たら、検査を広げる。
- `spikes/draw` の C ヘッダは、Echode が書き出す関数の手書きで、OS の宣言ではない。測定当時のまま残す。製品の橋渡しのヘッダは cbindgen が作る。
- 実機の HAL と第三者プラグインでの Workgroup の参加は、従来どおり `make spike-isolation`（ユーザーの依頼時だけ）で確かめる。今回は実行していない。
