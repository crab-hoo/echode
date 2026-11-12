# 0328. 作業の段階の Cargo の呼出しを既定のメンバーの 1 つの選択にそろえる

- 日付: 2026-11-12
- 状態: 採用
- 関連: [ADR 0093](0093-product-contract-gate.md)、[ADR 0212](0212-verification-tiers.md)、[ADR 0271](0271-task-tier-duplicate-runs.md)、
  [ADR 0276](0276-priority-order-and-delegated-decisions.md)（優先順位）、[ADR 0327](0327-instrumented-builds-only-on-github.md)
- 置き換え: [ADR 0148](0148-verification-pipeline-speed.md) の「Rust の契約は 1 契約 1 プロセスの `cargo test -p` で実行する」を作業の段階について。
  ADR 0148 の見送り「`check-base` の結果を契約の通常実行に流用しない」の理由（`--workspace` と `-p` の構成の違い）

## 背景

ユーザーは 2026-11-12 に、手元の Mac のストレージと CPU の負担を減らすよう依頼した（ADR 0327）。
同じ日の `target/debug` は 38 GB（`incremental` 20 GB、`deps` 17 GB）で、空きは 17 GiB（使用率 97%）だった。
`target/debug/incremental` には、同じ日のうちに `echode_plugin_host` の写しが 71 個、`echode_engine` が 70 個あった。

Cargo は、コマンドが選んだパッケージだけで feature を解決する。
`-p` の組が違うと、第三者の crate（serde_json、syn、objc2、num-traits など）の feature が変わり、その上のワークスペースの crate も別のハッシュの写しになる。
`cargo test --no-run --message-format=json` で、`echode_engine` の rlib のハッシュは `--workspace`、`-p echode-engine`、`-p echode-plugin-host` で互いに違った。
Rust を 1 行編集するたびに、写しの数だけコンパイルし直していた。
`resolver.feature-unification = "workspace"` は、安定版の cargo 1.96.1 では無視される（`-Zfeature-unification` が要る）。

作業の段階の `-p` の組は次のとおりだった。

- `check-base`: `--workspace` のビルド・clippy・テスト、`-p echode-actions` の例、`echode-history` と `echode-engine` の `spike`
- `check-swift`・`contract-prerequisites`: `-p echode-ffi -p echode-plugin-host -p echode-test-plugin -p echode-cli`
- `check-shell.sh`・`check-real-hal`: `-p echode-ffi`。`make bundled-plugins`: `-p echode-sine-pad`
- `check-product-contracts`: 契約のパッケージごとの `cargo test -p`（11 パッケージと `spike` の 1 組）

調べる中で、次の 3 つも分かった。

1. `--workspace` は製品の構成でもない。橋渡しスパイクの uniffi が `cargo_metadata` を通して serde_json の `unbounded_depth`（再帰の上限を外す）を有効にする。
   serde_core の `default`、syn の `full`・`visit-mut` も同じ経路で入る。`make check` の全テストは、アプリにないこの構成で動いていた。
2. Swift のテストがリンクする `libechode_ffi.a` は、出荷するものと別のビルドだった。
   `check-shell.sh` の `-p echode-ffi` では num-traits に `libm` がなく、`make app` の `-p echode-ffi -p echode-plugin-host -p echode-cli` にはある。
3. 新しい作業ツリーの冷えたビルドで、`EchodeBridgeTests` のリンクが `_NSApplicationMain` の未定義で失敗した。
   `rustc --print native-static-libs` は `libechode_ffi.a` に AppKit などを求めるが、`swift/EchodeBridge/Package.swift` は CoreAudio と AudioToolbox しか宣言していなかった。
   この作業ツリーではそれまで通っていた。どのオブジェクトを取り込むかがコードの配置で変わるためである。

## 比べた案

| 案 | 判断 |
| --- | --- |
| A. `resolver.feature-unification` / `-Zfeature-unification` | 安定版は無視する。製品のツールチェインで nightly の機能や `RUSTC_BOOTSTRAP` を使うことになるので採らない |
| B. 全部を `--workspace` にし、契約はそのバイナリで実行する | 橋渡しスパイクの serde_json の `unbounded_depth` が製品の crate に入る。契約の証拠がアプリにない構成を表すので採らない |
| C. 製品のパッケージの `-p` の一覧を Makefile とスクリプトで共有する | ビルドはそろうが、テストのない crate を外す一覧を別に保つことになる。一覧の重複が残る |
| D. `-p` の組ごとの feature の違いをなくす | 開発用依存が第三者の crate に feature を足す（rand の `std`、regex-syntax の `unicode`）。製品を変えずには消せない |
| E. cargo-nextest でビルドを共有する | 新しいツールの導入になり、libtest の出力で作る証拠の判定を作り直すことになる |
| F. Cargo の既定のメンバーを 1 つの選択にする（採用） | 選択の定義が `Cargo.toml` の 1 か所になり、`-p` を付けない呼出しは全部同じビルドを共有する |

## 決定

### 既定のメンバー

- `Cargo.toml` の `default-members` は `crates/` の全メンバーで、橋渡しスパイクだけを外す。
- 作業の段階の Rust のビルド・clippy・テスト・例の実行と、`make app`・`make dist`・`make bundled-plugins` は `-p` を付けない。
  手動・全体の段階の `known-bugs`・`bless`・`coverage`・`spike-*` の debug のビルド、サニタイザの前提も同じにした。
- `-p` は、feature を足す時（`spike`）と、既定のメンバーの外の橋渡しスパイクにだけ使う。
  `check-base` は橋渡しスパイクの clippy とテストを `-p echode-bridge-spike` で別に行う（`--workspace` が含めていた分）。
- release の `spike-*` 計測と GitHub の `check-portable` は対象外にした。どちらも `target/debug` を使わない。

### 2 つの写しと、契約の証拠が表す構成

- 写しは、出荷するビルド（`cargo build`）と、開発用依存の feature を足すテストのビルド（`cargo test`）の 2 つである。`spike` の構成は別に残る。
- テストのビルドが出荷するビルドに足す feature は次だけである。`scripts/tests/test_cargo_selection.py` の `TEST_ONLY_FEATURES` が一覧を持ち、それ以外を拒否する。
  - `echode-audio-io` の `headless-output`（`echode-document` と `echode-ffi` のテスト用の代役。`cfg(test)` でも有効）
  - rand・rand_core の `std`・`os_rng`・`alloc`（proptest）
  - regex-syntax の Unicode の表（proptest と insta）。製品の gix の正規表現も Unicode のクラスを受け付けるようになる。
    以前の `-p echode-history` などのテストのビルドも同じだった
- 作業の段階の Rust の契約の証拠は、この「出荷するビルドと、一覧の feature」の構成を表す。`make check` の Rust のテストと同じバイナリである。
- 全体の段階（GitHub の `check-contract-controls`）は、契約ごとの `cargo test -p` と、その構成での反例を保つ。
  記録の `build` は作業の段階が `members`、全体の段階が `package` で、どちらも相手の Rust の記録を再利用しない。
- Swift の契約と `check-swift` は、出荷するビルドの `libechode_ffi.a`・ヘルパー・CLI・検査用プラグインを使う。`make app` と同じ選択である。

### 契約の実行

- 作業の段階は、`cargo test --no-run --message-format=json-render-diagnostics` で選択ごとに 1 回ビルドし、報告から契約のバイナリを探す。
  既定のメンバーの選択は `check-base` の `cargo test` と同じ単位なので、通常は何もコンパイルしない。
  feature を名指す契約は、`check-base` の `cargo test -q -p echode-engine --features spike --lib` と同じ選択を使う。単体テストがこの一致を照合する。
- バイナリは `cargo test` と同じくパッケージのフォルダから起動し、`CARGO_MANIFEST_DIR` と `CARGO_PKG_NAME` を渡す。
  2026-11-12 の時点で、実行時に Cargo の環境変数を読むテストはなく、テストのバイナリは OS のライブラリだけを動的に読む。
  欠けた動的ライブラリは読込みの失敗になり、合格の証拠にはならない。
- 証拠の判定は変えない。合格の行が 1 回だけあること、無視・0 件・削除・二重の一致の拒否、まとめて示せなかったテストの単独の再実行（ADR 0093、0271）。
  ビルドが失敗した時や、報告にバイナリがない時は、その契約を理由とビルドのログ（`product-contracts/rust-test-build-*.log`）付きで失敗にする。
- Swift の契約を Rust より先に実行する。`cargo test` は `target/debug` のヘルパーと CLI を自分のビルドで置き換えるが（`libechode_ffi.a` は置き換えない）、
  Swift のテストは `contract-prerequisites` が作った出荷するビルドのものを起動する必要がある。

### 入力とリンク

- どのメンバーの依存も共有のビルドの feature を変えるので、`check-swift`・`check-real-hal`・契約の共通の入力に `crates/*/Cargo.toml` を足した。
- `swift/EchodeBridge/Package.swift` の `CEchodeFfi` は、`native-static-libs` の一覧（AudioToolbox、CoreAudio、CoreServices、AppKit、Foundation、CoreFoundation、iconv、objc）を宣言する。

## 測定

MacBook Air M4（10 コア）、2026-11-12。変更前（HEAD `26d6489` にリンクの修正だけを足した版）と変更後を、それぞれ新しい作業ツリーで測った。
ルートの `Cargo.toml` に注釈を 1 行足して作業の段階の全検査を選び、冷えた `target` から `make verify` を 1 回実行した。
続けて `crates/echode-engine/src/lib.rs` に注釈を 1 行足し、もう一度 `make verify` を実行した。

| 項目 | 変更前 | 変更後 |
| --- | --- | --- |
| 冷えた状態からの `make verify` | 1,231 秒 | 1,047 秒 |
| うち `check` / `check-swift` / `check-product-contracts` | 465 / 378 / 206 秒 | 462 / 351 / 56 秒 |
| その後の `target/debug`（`incremental` / `deps`） | 13 GB（4.9 / 7.1 GB） | 8.0 GB（2.9 / 4.4 GB） |
| `target/debug/incremental` の項目数 | 482 | 385 |
| `echode_engine` の `incremental` の写し / `deps` の rlib | 15 / 8 | 8 / 2 |
| 1 行の編集の後の `make verify` | 449 秒 | 405 秒 |
| うち `check` / `check-swift` / `check-product-contracts` | 268 / 52 / 75 秒 | 264 / 47 / 44 秒 |
| 編集の後の `target/debug` | 14 GB | 8.6 GB |

契約の通常の実行は、冷えた状態で 200 秒から 51 秒に、編集の後で 70 秒から 39 秒になった。
11 個の `-p` の組の写しをコンパイルしなくなった分である。`check` の時間はほぼテストの実行で、変わらなかった。
編集の後も `incremental` の項目数は変わらず、同じ写しを使い回した。
変更前の 38 GB と 70 個の写しは、1 日の多くの検査と `Cargo.lock` の更新で写しがたまった結果で、1 回の検査では 15 個だった。
`echode_engine` に残る 8 個は、出荷・テスト・clippy の別と、`spike` の構成の別である。`-p` の組による写しではない。
main の作業ツリーでも、空にした `target/debug` から 1 回の `make verify` の後に 7.9 GB だった（Swift のビルドは温まっていたので、時間は比べない）。

## 見送ったもの

- `spike` の 2 つの構成を 1 つにまとめる案と、`echode-engine-spike` のビルドをテストの構成へ寄せる案。
  まとめると片方の crate の `spike` がもう片方のテストに入り、各テストの意味が変わる。
- 全体の段階の契約を既定のメンバーのビルドへ寄せる案。反例は変えたコピーごとにビルドし、`-p` のほうが短い。GitHub は毎回新しいビルドなので写しの数は問題にならない。
- Swift のテストのバンドルが `libechode_ffi.a` の変更で再リンクされることの検査の追加。
  SwiftPM はリンカーの依存情報で追う。`libechode_ffi.a` を消すと再リンクを試みて失敗することを確かめた。

## 結果と制約

- ルートで `cargo build` / `cargo test` を実行しても橋渡しスパイクは入らない。橋渡しスパイクは `-p echode-bridge-spike` で扱う。
- 橋渡しスパイクの clippy とテストは、`echode-model` と `echode-time` の小さな写しを別に作る。
- `make app`・`make dist` は既定のメンバー全体をビルドする。出荷しない fuzz の lib と検査用プラグインも作る。Sine Pad は以前から別の呼出しで作っていた。
  代わりに、出荷するものの feature はテストの構成と同じになる。
- 依存の更新でテストのビルドの feature の差が変わると、`test_cargo_selection.py` が止める。製品の動作を変えない理由を書いて一覧へ足すか、依存を直す。
- 作業の段階の契約は、契約のパッケージだけで解決した構成を確かめない。それは全体の段階が GitHub で確かめる。

## 回帰・検査基盤

- `scripts/tests/test_cargo_selection.py`: 作業の段階の各検査のレシピと、それが動かすシェルスクリプトの `cargo` の呼出しが `-p`（feature なし）や `--workspace` を使わないこと。
  `make app`・`make dist` と Swift の検査が同じ選択でビルドすること、既定のメンバーから外れるのが橋渡しスパイクだけであること、
  テストのビルドの feature の差が一覧のとおりで serde_json の `unbounded_depth` がないこと、メンバーの `Cargo.toml` を入力に含めること。
- `scripts/tests/test_product_contracts.py`: バイナリを 1 回のビルドの報告から探し、パッケージのフォルダから起動すること、
  ビルドの失敗とバイナリの欠落を契約ごとに理由付きで失敗にすること、feature を名指す契約の選択が `check-base` の呼出しと一致すること、
  Swift の契約が Rust のビルドより先に動くこと、各段階が自分の `build` の記録だけを再利用すること。
