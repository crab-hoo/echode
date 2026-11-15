# ADR 0372: release ビルドで行番号表と版別 dSYM 保管を有効化し、panic.ndjson をオフライン解決可能にする

- 日付: 2026-11-15
- 状態: 現行
- 領域: ビルド・検査・開発基盤
- 関連: [PCA-06](../reviews/2026-11-13-professional-cross-audit.md#pca-06-release-の設定が既定のまま)、[ADR 0061](0061-distribution-signing.md)、[ADR 0238](0238-audio-callback-panics.md)、[ADR 0276](0276-priority-order-and-delegated-decisions.md)

## 背景

[PCA-06](../reviews/2026-11-13-professional-cross-audit.md#pca-06-release-の設定が既定のまま) の指摘どおり、ワークスペースの `Cargo.toml` に `[profile.release]` がなく、
`.cargo/config.toml` は `strip = "none"` のみを置いていたため、release ビルドはデバッグ情報なし・`.dSYM` 保管なしだった。
実機や第三者プラグインでのみ起きる異常終了では、macOS のクラッシュレポートと `panic.ndjson` のバックトレースが関数名止まりとなり、
発生行を特定できなかった。

[ADR 0276](0276-priority-order-and-delegated-decisions.md) の優先順位（堅牢性 > 保守性 > 効率 > 見た目・機能）に基づき、
最適化と実行時安全の前提（[E4](../status.md) で測る整数あふれ検査と `panic = "unwind"` の維持）を崩さずに、
配布バイナリを肥大化させない診断情報の分離・保管・復元手順を一貫して導入した。

## 比べた案

### release のデバッグ情報形式（`Cargo.toml`）

- **案 1: `debug = "full"` / `split-debuginfo = "off"`**:
  Mach-O 内に `__DWARF` セグメントが埋め込まれ、配布バイナリと `.app` が大きく肥大化する。
- **案 2: 既定のまま（デバッグ情報なし）**:
  バイナリは最小だが、利用者のクラッシュ記録と `panic.ndjson` から行番号を復元できない。
- **案 3（採用）: `[profile.release]` に `debug = "line-tables-only"` と `split-debuginfo = "packed"` を設定する**:
  最適化（`opt-level = 3`）や音声経路の動作を変えず、行番号解決に必要な DWARF 行番号表だけを独立の `.dSYM` バンドルへ分離し、
  リンク済みバイナリ本体へ `__DWARF` セグメントを持たせない。整数あふれ検査の採否は予定どおり E4 に残す。

### dSYM の保管と検証（`scripts/build-dist.sh`・`scripts/distribution.py`）

- **案 1: `target/release/` の `.dSYM` をそのまま残すだけにする**:
  次回ビルドや `cargo clean` で上書き・消失し、過去配布版の `LC_UUID` と照合できる形で保存されない。
- **案 2（採用）: `make dist` が署名後に全 4 バイナリの `.dSYM` を版・ビルド・コミット単位で保管し、UUID 一致と `__DWARF` なしを検査する**:
  `target/dist/dSYMs/<version>-<build>-<commit>/`（作業木が汚れている時は末尾に `-dirty`）へ、一時領域で検証と `manifest.json` 作成を終えてから原子的に配置する。
  対象は `Echode`・`echode-cli`・`echode-plugin-host`・`EchodeSinePad` の 4 バイナリである。
  あわせて `scripts/build-dist.sh` の `swift build -c release` へ `--product Echode` を指定し、
  `@testable import EchodeAppCore` を含む補助テストターゲット `EchodePlayheadChecks` をリリース条件で誤コンパイルしないようにした。

### `panic.ndjson` の解決情報（`crates/echode-audio-io/src/panic_log.rs`・`distribution.py symbolicate`）

- **案 1: 既存の `{backtrace}` 出力のみを維持する**:
  `.dSYM` のない実機環境では `Display` 表現にアドレスや行番号が出ず、保管済み `.dSYM` へ後から `xcrun atos` を当てられない。
- **案 2（採用）: 非コールバックの通常 `thread` panic のみ `{backtrace:#}` と dyld `images` を記録する**:
  オーディオスレッドのコールバック経路（512 バイト固定領域へのコピー・非確保、ADR 0238）は一切変えず、
  それ以外のスレッドで起きた panic のみ `Backtrace::force_capture()` を `{:#}` で記録して 16 進アドレスを残し、
  `mach2::dyld` から非システムイメージ（先頭イメージ、および `/System/` と `/usr/lib/` 以外）の `path`・`load`・`slide` を `images` 配列へ記録する。
  `scripts/distribution.py symbolicate --dsyms <dir> --panic-log <file>` で `manifest.json` およびロードアドレスと照合し、`xcrun atos` で行番号を復元する。
  なお、Cargo の `split-debuginfo = "packed"` は `<bin>.dSYM/Contents/Resources/DWARF/<bin>-<hash>` を作るため、
  `atos -o` には `DWARF/` 配下のファイル名ではなく `.dSYM` バンドル自身を渡す。

## 決定

1. [ワークスペース設定](../../Cargo.toml) に `[profile.release]`（`debug = "line-tables-only"`、`split-debuginfo = "packed"`）を追加した。
2. `crates/echode-audio-io/src/panic_log.rs` に `LoadedImage`（`path`・`load`・`slide`）を追加し、通常スレッドの panic で `{backtrace:#}` と非システム `images` を出力するようにした。コールバック経由の記録は `images` を省略する。
3. [配布スクリプト](../../scripts/distribution.py) に `macho_uuids`・`has_embedded_dwarf`・`archive_dsyms`・`symbolicate_panic_log`（CLI サブコマンド `archive-dsyms`・`symbolicate`）を追加し、[配布ビルド手順](../../scripts/build-dist.sh) から署名後に全 4 バイナリの `.dSYM` 保管と検査を必須実行するようにした。
4. [ADR 0238](0238-audio-callback-panics.md)、[API と配布の設計書](../architecture/api-qa-and-distribution.md)、[音声とプラグインのデバッグ手順](../debugging/audio-and-plugins.md)、[検証とビルドのデバッグ手順](../debugging/verification-and-build.md) を更新した。

## 測定と検証

- **バイナリ・dSYM サイズ（`scripts/build-dist.sh` 一時キーチェーン署名での実測）**:
  - `target/release/echode`: `8,658,088` B（変更前） → `9,541,352` B（変更後・未署名）、署名済み `echode-cli` `9,504,032` B、`echode-cli.dSYM` `36,472` KiB。
  - `target/release/echode-plugin-host`: `930,976` B（変更前） → `1,064,976` B（変更後・未署名）、署名済み `1,077,488` B、`EchodePluginHost.app.dSYM` `4,652` KiB。
  - `target/release/libechode_sine_pad.dylib`: `501,624` B（変更後・未署名）、署名済み `EchodeSinePad` `516,928` B、`EchodeSinePad.clap.dSYM` `2,428` KiB。
  - 署名済み `Echode.app/Contents/MacOS/Echode`: `17,945,760` B、`Echode.app.dSYM` `57,088` KiB。
  - 全 4 配布バイナリで `otool -l` の `segname __DWARF` は 0 件、全 4 件の `LC_UUID` が保管済み `.dSYM` と一致。
    `target/release/` の約 10% 増は `.cargo/config.toml` の `strip = "none"` により最小限のシンボル表を保持するためであり、DWARF 本体はバイナリへ入らない。
- **`xcrun atos` による行番号復元**:
  - 保管済み `Echode.app.dSYM` から `lib.rs:422`、`echode-cli.dSYM` から `panic_log.rs:37`、`EchodePluginHost.app.dSYM` から `main.rs:8`、`EchodeSinePad.clap.dSYM` から `entry.rs:344` の行番号解決を確認した。
- **回帰テストと基盤保守**:
  - `crates/echode-audio-io/src/panic_log_tests.rs` に 16 進アドレス・非システム `images`・コールバック時の `images` 非出力の検証を追加した。
  - `scripts/tests/test_distribution.py` に release バイナリの `__DWARF` 非含有・`.dSYM` 分離、保管 dSYM からの `panic.ndjson` 行番号復元、`archive-dsyms` による UUID 不一致およびバンドル内 `.dSYM` 混入の拒否を追加した。
