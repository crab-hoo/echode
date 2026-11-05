# 0269. GitHub で公開し、CI の範囲を決める

- 日付: 2026-11-05
- 状態: 採用。同日の[追記](#2026-11-05-追記-全体の段階を-github-で実行する)で、全体の段階を CI へ移した
- 関連: [ADR 0044](0044-swift-bridge.md)、[ADR 0114](0114-m4-build-baseline.md)、[ADR 0212](0212-verification-tiers.md)、
  [横断監査](../reviews/2026-11-02-cross-cutting-audit.md) の XCA-04・05・22
- 置き換え: ADR 0044 の CI の節と、ADR 0114 の「CI のネイティブ検査」の記述

## 背景

リモートを Cursor Origin から GitHub へ移し、公開リポジトリにする。
これまでのワークフローは `verify.py --github-matrix` で全自動検査を分担していたが、一度も実行されていない（XCA-05）。

2026-11-05 に確認した GitHub の実行環境:

- `xcode-27` ラベルは 2026-10-15 に一般提供となり、macOS 27 と Xcode 27 で動く。
- 標準の macOS ランナーは M1（3 vCPU）、`-xlarge` は M2 で有料である。M4 のランナーはない。
- `check_build_host.py` は M4 以降を要求するので、旧ワークフローのネイティブ検査は最初の手順で必ず失敗する。

自分の M4 をセルフホストランナーにする案は採らない。
公開リポジトリでは、フォークの pull request が利用者の Mac で任意のコードを実行できる。

公開は匿名で行う。履歴の作者メールには、ホスト名と所属のネットワーク名が入っていた（XCA-22）。
監査の再現資料にも、マシンのホスト名とホームディレクトリのパスが残っていた。

## 決定

### CI の範囲

- `.github/workflows/check.yml` は `make check-portable` と `make advisories` だけを実行する。
- `make check-portable` は、フォーマット、`scripts/tests`、FFI、ライセンス、clippy を含む。
  テストと製品バイナリの実行はしない。
- clippy は `target-cpu=apple-m1` を設定ファイルの値の後ろへ足してコンパイルする。
  ビルドスクリプトと手続きマクロが M1 / M2 の上で M4 の命令を実行しないためである。
  ローカルの M4 の結果を消さないよう、出力先は `target/portable` に分ける。
- cargo を呼ぶジョブ（cargo-deny を含む）は、最初に `rustup toolchain install` で `rust-toolchain.toml` の版を入れる。
  ランナーの rustup は、この固定版を自動では入れない。
- CI の合格は `make verify` の合格ではなく、`target/verification/report.md` にも記録しない。
  ネイティブの検査、サニタイザ、反例はローカルの M4 の `make verify` / `make verify-full` のままにする。
- `--github-matrix` は使う場所がなくなったので削除した。

### 公開リポジトリの防御

- トークンの権限は `contents: read` に限り、`pull_request_target` を使わない。
- アクションは 40 桁の SHA で固定し、コメントに版を書く。更新は手で行う。
- `push` は `main` だけで起動し、pull request との二重実行を避ける。advisory は週 1 回も実行する。

### 履歴の書き換え

2026-11-05、プッシュの前に `git filter-repo` で全履歴を書き換えた。

- 作者とコミッタを `crab hoo <219308278+crab-hoo@users.noreply.github.com>` に統一した。
  このリポジトリの `user.name`・`user.email` も同じ値にした。
- ファイル内容のホスト名を `build-host`、ホームディレクトリを `/Users/someone` に置き換えた。
  名前につながる文字列は、全履歴でこの二つだけだった。コミットメッセージには含まれていなかった。
- `.codex/hooks.json` のコマンドは、絶対パスをやめて `git rev-parse --show-toplevel` から解決する。
- 全コミットの SHA が変わった。文書の SHA 参照は新しい値へ置き換えた（[証拠の保全](../evidence/README.md#2026-11-05-の履歴書き換え)）。

## 結果と制約

- CI の clippy は製品と異なる CPU でコンパイルする。
  2026-11-05 時点で `cfg(target_feature)` を使うコードはないが、追加すると CI はその分岐を検査しない。
- ランナーの機種とイメージは GitHub の都合で変わる。M4 のランナーが提供されたら、この判断を見直す。
- 固定したアクションは自動では更新されない。
- 2026-11-05 の初回実行は、非公開のリポジトリへプッシュした `f85dcae` の旧ワークフローで起きた（実行 41981233638）。
  - ネイティブ検査の 14 ジョブは、`xcode-27-arm64`（20261104.0337）の上で `check_build_host.py` が止めた。
    出力は `got 27.0.1, arm64, Apple M1 (Virtual)` で、背景の前提どおりである。
  - advisory は `macos-15` の上で `toolchain '1.96.1-aarch64-apple-darwin' is not installed` により失敗した。
    新しいワークフローにも同じ欠落があったので、上の導入手順を足した。
- 新しいワークフローの初回（`34fb2da`、実行 41992484857）では、advisory が合格した。
  `check-portable` は `scripts/tests` の署名の検査 1 件で止まり、以降の FFI・ライセンス・clippy は動かなかった。
  主アプリ（hardened runtime、`disable-library-validation` なし）がアドホック署名のライブラリを読み込んだためである。
  ランナーの VM はライブラリ検証を強制していないとみられるが、ランナー上では確かめていない。
- そのため署名の検査に対照を入れた。hardened runtime でアドホック署名した読込み役が同じライブラリを読み込めたら、そのホストは検証を強制していない。
  GitHub Actions ではその時だけ、主アプリの拒否の部分を理由付きで skip する。手元の Mac では失敗にする。
  CI はこの拒否を検査せず、手元の M4 の `make verify` だけが検査する。
- 修正後の GitHub 上での実行は未確認である。

## 回帰・検査基盤

- `test_public_ci_runs_only_pinned_static_checks` が、権限、SHA の固定、`check-portable` の中身を検査する。
  タグで指定したアクションで失敗することを確かめた。
- `test_public_ci_installs_the_pinned_toolchain_before_cargo` が、cargo か make を呼ぶ各ジョブで、その前に固定版を入れることを検査する。
  導入手順のない advisory のジョブで失敗することを確かめた。
  ランナーの失敗は、空の `RUSTUP_HOME` と `RUSTUP_AUTO_INSTALL=0` で `make advisories` を実行すると、同じ出力と終了コード 2 で再現する。
  導入手順を足した後は、同じ条件で合格した。
- `test_published_files_name_no_home_directory` が、追跡・未追跡のファイルにある `/Users/<名前>` を拒否する。
  プレースホルダーの `/Users/someone` と `/Users/Shared` だけを許す。置き換え前の作業ツリーで失敗することを確かめた。
- ローカルでは `make check-portable` を M4 の上で実行した。M1 / M2 での実行は CI の初回で確かめる。

## 2026-11-05 追記: 全体の段階を GitHub で実行する

### 背景

手元の Mac は空きが 14 GB（使用率 97%）で、`target/` が 39 GB を使っていた。
内訳は `target/debug` 19 GB、反例用の `target/product-contract-mutations` 8.5 GB、サニタイザ用の `target/bridge` 7.3 GB。
ユーザーから「GitHub でできる検査はできるだけそちらで行い、手元の容量を抑えたい」と指定された。
次の検査が速くなるなら、ある程度の容量は許すとも指定された。

### 決定

- `.github/workflows/full.yml` が全体の段階の 5 件を実行する。
  ASan、TSan と errno の検査用ランタイム、Miri、全契約の反例である。
  `main` へのプッシュ（`docs/` と Markdown だけの変更を除く）と手動起動で動く。
- ビルドは ADR 0114 の apple-m4 のままにし、M1 のランナーで実行する。
  手元の M4 で作った Mach-O 150 個を `llvm-objdump -d --mcpu=apple-m1` で逆アセンブルした。
  Rust のテストと製品バイナリが 125 個、Swift のアプリ・テストバンドル・プローブとモジュールのオブジェクトが 25 個である。
  `--mcpu=apple-m4` より `<unknown>` が多いものはなかった。
  対照として、I8MM の `vmmlaq_s32` と BF16 の `vcvt_bf16_f32` を apple-m4 でコンパイルすると、apple-m1 では `<unknown>` になった。
- CI だけ apple-m1 でビルドする案は採らない。
  Package.swift 4 個、`build-platform.env`、`force = true` の `CFLAGS`、TSan のランタイムなど 15 か所以上に切替えが要る。
  そのうえ、CI が手元と違うバイナリを検査することになる。
- M4 にしかない命令が後で入ると、そのテストは `Illegal instruction` で落ちる。黙って合格にはならない。
  反例の判定はテスト本体の失敗（Rust の `FAILED` 行、Swift の `Expectation failed`）を要求するので、この落ち方を検出とは数えない。
  その時は、CI だけ apple-m1 でビルドする切替えを足す。
- 反例は `ECHODE_CONTRACT_SHARD=I/N` で 4 つのジョブに分ける。
  各ジョブは必須テストを全部実行してから、契約の並びで I 番目から N 個おきの反例を実行する。
  手元の M4 では全件で 2,481 秒だった。
- サニタイザは 3 つのジョブに分け、それぞれで `make setup-bridge` を実行する。
  TSan のランタイムは毎回ビルドする。キャッシュにするかは、実行時間を見て決める。
- `scripts/check_build_host.py` は変えない。全体の段階の make の入口はこれを呼ばず、CI では `verify.py` を使わない。
  ADR 0114 の「CI のネイティブ検査は実行前にチップを調べる」は、この追記で置き換える。
- CI の合格は、引き続き `report.md` に記録しない。
  節目（依頼・配布前・受入）では、対象のコミットで `full` が合格したことを `gh run list` で確かめる。
  手元の `make verify-full` は、依頼された時か、CI で再現しない失敗を調べる時に使う。
- 作業の段階（`make verify` と停止フック）は手元のままにする。
  エージェントは頼まれない限りプッシュしないので、CI に任せると完了の前に検査できない。

### 手元の容量

- `Cargo.toml` に `[profile.dev.package."*"] debug = "line-tables-only"` を足した。
  依存ライブラリは、バックトレースのファイルと行だけを残す。ワークスペースの crate は、lldb のために完全なデバッグ情報を残す。
  `libechode_ffi.a` は 350 MB から 271 MB に、そのうちのデバッグ情報は 228 MB から 149 MB に減った。
  ワークスペースのテスト一式のビルドは 4.0 GB で、M4 で 1 分 49 秒だった。
- 古い成果物は別のハッシュになり再利用されないので、`target/debug`、`target/product-contract-mutations/cargo`、`target/bridge` を消した（約 34 GB）。
  TSan のランタイム（`target/toolchains`）、検査の記録（`target/verification`）、反例のソースのコピーは残した。

### 結果と制約

- 標準のランナーは 3 vCPU で、待機期限が 5 秒のテストがサニタイザの下で遅れるおそれがある。
- ランナーで Metal を使えるかは確かめていない。描画を使う必須テストが落ちたら、まずこれを疑う。
- 初回（`dcb047a`、実行 42005867712）では、反例の 4 シャードが 24.9〜27.9 分、Miri が 3.2 分で合格した。
  反例のジョブでは、Rust 110 件と Swift 146 件の必須テストも M1 のランナーで合格した。
  ランナーのディスクは 180 GB で、TSan のジョブの後も 32 GB 空いていた。
- 同じ実行で、ASan は EchodeApp のテストが `Export CLI is missing from the app bundle` で失敗した。
  サニタイザの入口は `target/debug` の CLI・プラグインヘルパー・検査用プラグイン・同梱プラグインをビルドしていなかった。
  手元では、先に `check-swift` が作った物がいつも残っていたので表に出なかった。
  `check-bridge-asan` と `check-bridge-tsan` の入口で、`check-swift` と同じ物を先にビルドするようにした。
  入力には、新しく使う `scripts/build-bundled-plugins.sh` と同梱プラグインの plist を足した。
  CLI などの crate は入力に足さない。サニタイザはそれらを計装しないので、そこでの `unsafe` の変更で選ばれても検査にならない。
  その crate の変更で手元の合格の記録は失効しないが、`full` は `main` へのプッシュごとに作り直して実行する。
- TSan も同じ Swift のテストで失敗したが、ジョブのログには失敗ログの末尾の dyld の出力しか残らず、原因を確定できなかった。
  失敗した時に、失敗ログから問題の行だけをジョブのログへ出す手順を足した。
- 作業の段階でも、`unsafe` の変更や検査自身のスクリプトの変更では、全体の段階の検査が手元で選ばれる。この選択は変えていない。

### 回帰・検査基盤

- `test_full_ci_runs_every_full_tier_check_with_complete_shards` が、`full.yml` の検査と `tier: full` の登録の一致、シャードの番号の抜けを検査する。
  検査を 1 件外した時と、シャードを 3 個にした時に失敗することを確かめた。
- `test_public_ci_installs_the_pinned_toolchain_before_cargo` は全ワークフローを対象にした。
- `test_shards_split_every_counterexample_once` と `test_a_shard_runs_every_test_but_only_its_own_counterexamples` が、シャードの分け方と実行範囲を検査する。
- `test_negative_controls_must_fail_in_the_test_body` に、`Illegal instruction` の終了を検出と数えない場合を足した。
- `test_sanitizer_checks_build_what_their_swift_tests_launch` が、サニタイザの入口がテストの起動する物を先にビルドすることを検査する。
  修正前の Makefile で `echode-cli` の欠落により失敗することを確かめた。
