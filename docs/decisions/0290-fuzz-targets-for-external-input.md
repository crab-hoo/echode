# ADR 0290: 外部入力に fuzz の対象を置き、カバレッジを測る

- 日付: 2026-11-09
- 状態: 採用。
- 対象: [統合表](../reviews/2026-11-08-open-items-register.md) の M3（[横断監査](../reviews/2026-11-02-cross-cutting-audit.md#xca-17-fuzz-とカバレッジ) の XCA-17）。
  fuzz と見つかった 2 件を先に、[カバレッジ](#カバレッジ) を後の作業で決めた。
- 方針: [ADR 0278](0278-audit-backlog-order.md) の「M3 を先にする。見つかった欠陥は堅牢性の段へ戻る」。
- 関連: [ADR 0212](0212-verification-tiers.md)（検査の段階）、[ADR 0269](0269-github-public-ci-scope.md)（CI）、[ADR 0288](0288-received-descriptors-are-all-owned.md)（記述子）。

## 問題

- fuzz の仕組みがなかった。性質テストは `echode-history` の操作列に限られ、外部の入力は手で選んだ反例だけを検査していた。
- XCA-17 が挙げた外部の入力は、ジャーナル、パッケージの JSON、音声の復号、ヘルパーの制御メッセージ、共有領域のイベントの 5 つである。
  どれも、壊れた・悪意のあるプラグインのヘルパー、壊れたディスク上の文書、利用者が取り込むファイルから届く。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | cargo-fuzz（libFuzzer）を固定 nightly の ASan で動かす | 採る |
| 2 | proptest の乱数入力を通常テストで流すだけ | 採らない。カバレッジで導かれず、形式の深い分岐（JSON の後の不変条件、`SCM_RIGHTS` の位置）に届きにくい。ASan もない |
| 3 | bolero など、複数のエンジンを包む crate | 採らない。依存が増える。通常テストでの再生は、案 1 でも同じ関数を呼べば足りる |
| 4 | AFL++・honggfuzz | 採らない。別のビルドとランタイムの導入が要り、macOS の対応は libFuzzer より弱い |

- 案 1 の依存は検査専用で、製品に入らない。cargo-fuzz 0.13.2 は MIT OR Apache-2.0、libfuzzer-sys 0.4.13 は (MIT OR Apache-2.0) AND NCSA である。
  NCSA は FSF が GPL と両立すると認める寛容なライセンスで、`deny.toml` の許可に足した。
- nightly は `scripts/bridge-toolchain.env` の固定版を共有し、2 つ目の固定を作らない。

## 決定

### 対象

`crates/echode-fuzz` を workspace に足した。各対象の本体は `Target::run` に 1 つだけ置く。
libFuzzer のバイナリ（`fuzz_targets/`）と通常テスト（`tests/replay.rs`）が同じ関数を呼ぶ。
バイナリは `libfuzzer` feature の時だけビルドし、通常のビルドとテストは libFuzzer をコンパイルしない。

| 対象 | 製品の入口 | 落ちる以外に確かめる性質 |
|---|---|---|
| `control` | `Socket::try_recv_tagged`。記述子を 0〜3 個、任意の位置に付ける | 受理した制御メッセージは符号化し直しても同じ。受けた記述子は全部閉じる |
| `region` | ヘルパーと同じ共有メモリへ任意のバイトを書き、本体の投入・回収・フラグ読みと交互に行う | 本体の世代は 1 以上。回収した件数は枠の数以下 |
| `journal` | 見本のパッケージの上で `journal.jsonl` を `recover` する | 開けた文書は不変条件を守り、保存して読み直すと同じ |
| `package` | ファイルの束を書いて `recover` する | 同上 |
| `audio` | 保管庫の PCM（`read_pcm`）と、拡張子 7 種のファイルの取込み・一括復号・逐次読み | 取込みの保管物は一括復号と同じ。復号した音は有限で、件数がフレーム数と合う |

- 種は製品の符号化と編集から作り、古いファイルを保存しない。形式が変わっても、種は今の形式のままである。
- 見本のパッケージの ID は時刻から作られ、プロセスごとに変わる。生成順を保つ固定の ID に置き換え、別のプロセスが作った種と基底が合うようにした。
- 制御の記述子は、パイプの書込み側の写しを送り、読み側が終端に届くかで残りを判定する（ADR 0288 と同じ）。
  記述子の数を数える方法は、通常テストの並行するスレッドで誤検出した。1 プロセスでは 1 入力ずつ動かす。
- 発見は最小化して `regressions/<対象>/` に置き、`check` の通常テストで毎回再生する。

### 検査の段階と CI

- `check-fuzz` を全体の段階に置く。ASan で各対象 60 秒、先に種と保存した発見を通す。GitHub の `full` に `fuzz` ジョブを足した。
- 作業の段階で選ぶのは `scripts/fuzz.sh` と `fuzz_targets/` が変わった時だけである。対象の本体の変更は通常テストの再生が受け持つ。
- `hook_timeout` は 15600 から 17460 秒にした（`check-fuzz` の 1800 秒と事前確認の 30 秒）。各フックの上限も合わせた。
- 長い探索は `make fuzz FUZZ_TARGET=名前 FUZZ_SECONDS=秒`、準備は `make setup-fuzz` で行う。

## 見つかった不具合

2026-11-09、Apple M4・macOS 27.0（26A428）・Apple clang 21.0.0 で、各対象を 60 秒動かした最初の実行で見つかった。

### 共有領域の世代を、ヘルパーが書ける値から決めていた

- 本体は次の世代を、共有メモリの見出しの `published` に 1 を足して決めていた。ヘルパーはこの語を書ける。
- `u64::MAX` を書かれると世代は 0 になり、デバッグビルドは音声スレッドの `debug_assert!` で止まった。
  大きな値を書かれると、`published − completed` が 2 以上になり、本体は再起動まで `Busy` で投入できなかった。
- 本体は自分で投入した世代を `issued` に持ち、見出しの `published` はヘルパー用の写しとして書くだけにした。読み戻さない。
- 回帰は `region_tests.rs` の `a_forged_published_counter_does_not_number_the_hosts_blocks`。修正前は `submit: Busy`、`completed` も偽ると `assertion failed: generation >= 1` で失敗した。
  `a_full_pair_of_slots_is_not_overwritten` は `published` を偽って 2 枠の使用中を作っていた。実際に 2 回投入する形に直した。

### 取り込む音声の確保を、ファイルが主張する長さで決めていた

- 一括復号（取込みと試聴の全体読込み）は、AudioToolbox が報告するフレーム数で `vec![0.0; 長さ × チャンネル]` を作っていた。
- 54 バイトの AIFF は、`SSND` の長さを 3.2 GB と書くと、約 8.1 億フレームと報告された。確保は 6,463,337,648 バイトだった。
  macOS は大きな確保を遅延して受けるので、この例では落ちない。さらに大きな主張では確保が失敗し、プロセスが中断する。
- 最初の確保は、主張とファイルの大きさで表せる量の小さい方にした。PCM はフレームの大きさ、圧縮はパケットが 1 バイト以上として数える。
  その後は復号した分だけ `try_reserve` で伸ばし、確保の失敗はエラーとして返す。復号器が求めた以上を返した時の拒否も、逐次読みと同じく足した。
- 正しいファイルでは主張がファイルの大きさを超えないので、最初の確保は従来と同じ大きさで、倍々の伸長は起きない。
- 回帰は `echode-history/tests/audio_claimed_length.rs`。1 回の確保の最大を測り、修正前は 6,463,337,648 バイトで失敗した。

### 対象の不備

- `package` が 255 バイトを超えるファイル名で止まった。製品ではなく対象の書込みの問題で、ファイルシステムが拒む名前は無いファイルとして扱うように直した。

## 測定

同じ機器・ASan で、修正の後に各対象を 3 分動かした。新しい発見はなかった。

| 対象 | 入力数 | 入力／秒 | 到達した辺 |
|---|---:|---:|---:|
| `control` | 478,799 | 2,645 | 1,148 |
| `region` | 497,283 | 2,747 | 744 |
| `journal` | 155,619 | 859 | 6,583 |
| `package` | 70,712 | 390 | 7,973 |
| `audio` | 21,907 | 121 | 1,035 |

- `audio` は 1 入力ごとにファイルを書き、AudioToolbox で 3 回開くので最も遅い。
- 同じ日の `make verify` で選ばれた `check-fuzz`（各 60 秒）も、新しい発見なく合格した。

## 結果と制約

- 60 秒は短い探索で、合格は欠陥がない証明ではない。長い探索は PLAN の Phase 6 に残る。
- AudioToolbox の内部は Apple のコードで、その不具合は製品では直せない。
- JSON-RPC の入力、MIDI の解析、プラグインの走査結果、波形キャッシュの読込みには、まだ対象がない。
- CI の M1 のランナーでは、libFuzzer の C++ も `.cargo/config.toml` の `-mcpu=apple-m4` でコンパイルされる。ADR 0269 と同じく、M4 だけの命令が出れば `Illegal instruction` で分かる。
- fuzz のバイナリの Rust は、cargo-fuzz が `RUSTFLAGS` を置くので `target-cpu=apple-m4` なしでビルドされる。製品のバイナリではない。

## カバレッジ

### 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | cargo-llvm-cov（rustc の `-C instrument-coverage`）と SwiftPM の `--enable-code-coverage` で行を測る明示の計測 | 採る |
| 2 | 1 と同じ測り方で、割合の下限を合否にする | 採らない。分岐を通るだけのテストでも割合は上がり、性質を確かめるテストを増やす動機にならない |
| 3 | grcov | 採らない。同じ LLVM の記録を読むだけで、テストの実行と記録の収集を別に組む必要がある |
| 4 | GitHub の `full` で毎回測る | 採らない。Rust と Swift の全テストをもう一度動かす。割合は節目に見れば足りる |

### 決定

- `make coverage`（準備は `make setup-coverage`）は、Rust では `check` と同じ `cargo build --workspace` と `cargo test --workspace` を計装して動かす。
  出力先は `target/llvm-cov-target` で、通常の `target/debug` は計装しない。
  `build-bundled-plugins.sh` は `CARGO_TARGET_DIR` に従い、テストがバイナリの隣に同梱の CLAP を見つけられるようにした。
- Swift は `EchodeBridge` と `EchodeApp` の `swift test --enable-code-coverage` を使う。Rust の静的ライブラリは計装しない `target/debug` のものをつなぐ。
- `scripts/coverage_summary.py` が、製品のソースの行を crate・Swift モジュールごとに数え、`target/coverage/summary.md` に書く。
  テストのディレクトリ、`*_tests.rs`、スパイク、例と、テストの道具の crate（`echode-fuzz`・`echode-test-plugin`）は数えない。
- テストが 1 件でも失敗したら計測を止める。失敗したテストの割合は誤解を招く。
- 登録簿では手動の `coverage` とし、どの変更でも選ばない。記録する時は `--run coverage` を使う。
- 道具は cargo-llvm-cov 0.9.1（Apache-2.0 OR MIT）と、固定した Rust の `llvm-tools` である。製品に入らない。

### 測定

2026-11-09、`623c068` の後の作業ツリー（同じ機器）で、全テストが合格した状態の行の割合である。

| 単位 | 行 | 実行した行 | 割合 |
|---|---:|---:|---:|
| Rust の製品の計（16 crate） | 36,579 | 30,828 | 84.3% |
| 最も低い crate: `echode-plugin-host` | 4,584 | 2,780 | 60.6% |
| `echode-audio-io` | 3,503 | 2,687 | 76.7% |
| `echode-ffi` | 1,676 | 1,324 | 79.0% |
| 最も高い crate: `echode-model` | 4,934 | 4,818 | 97.6% |
| Swift の計 | 16,253 | 11,844 | 72.9% |
| `EchodeAppCore` | 13,991 | 10,260 | 73.3% |
| `EchodeBridge` | 2,262 | 1,584 | 70.0% |

- 全 crate の値は `target/coverage/summary.md` にある。`make coverage` の実時間は 6 分 23 秒だった（計装した依存は前の実行のビルドを再利用）。
- helper のプロセスで動く行（`echode-plugin-host` の helper 側と `echode-sine-pad` の 0%）は少なく数える。
  本体は shutdown の応答の直後に待たずに確かめ、まだ終わっていない helper を SIGKILL で止める。helper は終了時に記録を書く前に止まる。
  製品の振る舞いとしては正しいので、計測のためには変えない。helper 側の割合が要る時は、helper の終了を待つ計測を別に作る。

### 結果と制約

- 割合は実行した行を数え、その行で正しさを確かめたかは示さない。
- 低い単位は、次にテストを足す場所の候補である。M1・M2 などで触る crate では、作業の前後に `make coverage` で比べられる。
