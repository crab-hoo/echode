# 0269. GitHub で公開し、CI は M4 を要らない静的検査に限る

- 日付: 2026-11-05
- 状態: 採用
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
- 新しいワークフローの GitHub 上での実行は未確認である。

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
