# デバッグ手順: 検査・ビルド・環境

検査基盤・ビルド・ツール・サニタイザーの失敗を、製品の不具合・環境・検査の不足に切り分ける。
最初の診断と記録する項目は [デバッグ手順](../debugging.md) にある。症状の一覧は `--outline` で見る。

## 検査の時間・期限・待ち続け

### `make verify` の所要時間の増加

`make verification-profile` で今回の実行・再利用と履歴、契約数・反例数を確認する。
旧レポートの最後の合格時間を、今回再利用した検査の実行時間へ足さない。
製品契約の `execution` は通常実行と反例、同期、ビルド作業秒と直列テスト秒を分ける。
契約の秒数は自身の処理時間であり、並列の作業秒の和は全体の実測ではない。
`normal_pending` と `negative_pending`、`reused_tests` と `reused_controls` で、通常・反例のどちらが失効したかを分ける。
依存先の統合テストだけの変更で Swift が失効する場合は `package_scope` と明示した `inputs`、build script・custom target を調べる。
Rust の製品ソース変更による Swift の失効はリンク先変更であり、反例の置換ファイルが不変という理由だけで全体の段階から除外しない。
`make verify` で多数の反例が走る時は、`execution` の `changed_sites` と置換箇所を確認する。
定義・置換元の前後 20 行・テスト本体が変わった契約だけが対象である（[ADR 0212](../decisions/0212-verification-tiers.md)）。
履歴は `target/verification/timing-history.jsonl` にあり、`make debug-report` にも収集する。
同じ条件での再計測と、件数・失効集合の変化は [時間の保守](../verification.md#実行時間の保守) に従って分ける。
Swift 契約の直接起動で `missing helper, test bundle, or Testing framework` が出た場合は、
選択した Xcode の `swiftpm-testing-helper` と SDK の Testing.framework、`--show-bin-path` の scratch を照合する。
ロード失敗・終了コード 69 は期待値での反例検出ではない。古いバンドルへ戻したり 0 件を合格にしない。
ヘルパーの実パス・バイト変更もツール識別に入り、現在の版で再検査する（[ADR 0198](../decisions/0198-verification-launch-and-cache-cost.md)）。
外側の実行ツールの期限だけで `make verify` が止まると、レポートが `running` のまま、子の `make check-base` が PPID 1 で残ることがある。
検査ログ・ロックの保持者・PID を照合し、残った自分の子プロセスの終了を待ってから同じ入口で再実行する。並列に重ねず、途中のログだけで合格を記録しない。
実行ツールの待機時間は検査全体を収める値にし、検査自身の期限は延ばさない。今回実行の所要時間と、終了済みの検査結果の再利用を区別する。

### `check-verification` の `VERIFICATION TIMEOUT`

ログが進捗の点だけで `FAIL:` / `ERROR:` と `Ran N tests` がなければ、テストの失敗ではなく 180 秒の期限である。
製品の不具合や変更の影響と扱わず、期限も緩めない。
`top` で他の作業の負荷を確認し、静かになってから再実行する。すぐに繰り返しても同じ負荷なら通らない。
2026-10-31 の短縮後は、負荷なしで約 60 秒、ビジーループ 7 本で約 108 秒である（[ADR 0208](../decisions/0208-verification-shared-scopes.md)）。
短縮前は同じ 7 本で 184〜186 秒かかり、OpenUtau が約 3 コアを使う時は 201 秒で止まった。
これより大幅に長ければ、`python3 -m unittest discover -s scripts/tests -v --durations 12` で遅いテストを確かめる。
実リポジトリの識別計算のテストが上位に戻った時は、範囲の共有が外れていないかを見る。
`contract_fingerprints` 1 回は約 0.6 秒で、`scope_entries` は契約 236 件に 11 回だけ呼ばれる。
短縮で足りなければ期限を変え、測定値と理由を ADR に書く（[ADR 0276](../decisions/0276-priority-order-and-delegated-decisions.md)）。

### `check` の `VERIFICATION TIMEOUT`

`check.log` に `FAILED`・`error` がなく、最後が進捗の点と `VERIFICATION TIMEOUT` なら、テストの失敗ではなく 1200 秒の期限である。
2026-11-11 は `echode-sandbox` の変更後の最初の実行が、最後の `echode-history` の spike のテストの途中で期限に達した。
直前に空き容量の不足（2.3 GiB）で事前確認が止まり、サニタイザのキャッシュを消して 5.5 GiB にした直後だった。ほかの作業の負荷はなかった。
作られた成果物は残るので、そのまま再実行すると `check` は 561 秒で通った。止まった段だけなら 47 秒である。
多くの crate が依存する crate の変更では再ビルドが広く、最初の実行が期限を超え得る。空きの少なさが遅さに効いたかは切り分けていない。
期限を緩めず、ログに失敗がないことを確かめてから再実行する。再実行も期限を超える時は、`check.log` のコマンドの行から遅い段を特定して測る。
同じ日の ADR 0317 でも、`echode-sandbox` の変更後に同じ段（`echode-history` の spike のテスト）で期限に達した。空きは 2.2 GiB だった。
11/8 から更新のない反例のキャッシュ（`target/product-contract-mutations` の 7.3 GB、全体の段階の `check-contract-controls` だけが使う）を消して 9.6 GiB にし、再実行で通った。
2 回とも、空きが 2〜3 GiB の時に、多くの crate が依存する crate を変えた後の最初の実行だった。

### Rust のテストで最初の音声ファイルの読込みが数秒〜十数秒かかる（`audition stalled at ... phase: Loading`）

2026-11-03 の `make verify` で、`rtsan` の試聴のテスト 3 件が 10 秒の待ちで落ちた。`handled: 0`・`source_frames: None` で、`AudioFileReader::open` の中だった。
RTSan の有無に関係なく、`target/debug/deps` の中のテストの実行ファイルでは、プロセスで最初の `ExtAudioFileOpenURL` が 6〜15 秒かかった。
`sample` の結果は `AudioConverterNewWithOptions` → `CAAppWorkArounds::CSCheckFix` → `CFBundleGetLocalInfoDictionary` → `_CFBundleReadDirectory` である。
bundle のない実行ファイルでは、置かれたフォルダが主 bundle とみなされ、その一覧を全部読む。この日の `target/debug/deps` は 429,278 件だった。
同じ実行ファイルを空のフォルダへ写すと 9.5 ms、`deps` の中では 8.6 秒だった。`afinfo` は 0.01 秒である。
製品のアプリは `.app` の bundle の中で動き、CLI の `target/debug` は 51 件なので、この遅れは出ない。製品の不具合でも RTSan の検出でもない。
試聴のテストは `warm_audio_toolbox`（`audition_tests.rs`）で最初の読込みを 1 回済ませてから、停止の期限を数え始める。
ほかのテストで同じ遅れが出たら、その実行ファイルを空のフォルダへ写して比べる。`deps` の古い成果物を減らすかはユーザーと決める。

2026-11-03 の後続確認では `deps` が 549,820 件あり、通常の `cargo test -q --workspace` も 1,200 秒で終わらなかった。`--no-run` は 1.178 秒だった。
同じ `device_offline` の製品回帰は、小さいフォルダのコピーで 1.098 秒、`deps` では 22.984 秒だった（単回の診断比較）。
`sample` は `ExtAudioFileOpenURL` → `AudioAnalytics` → `_CFCopySystemVersionDictionary` → `CFBundleGetMainBundle` → `_CFIterateDirectory` の列挙を示した。
テスト本体の失敗と混同せず、期限を延ばす前に置き場所を比較する。キャッシュの整理は下の空き容量不足の手順と ADR 0276 に従い、検査ログを消す `cargo clean` 全体実行は避ける。

#### 蓄積した `.rcgu.o` だけを整理する

2026-11-08 は、テストが始まった後に採った sample で、次の列挙を確認した。
`ExtAudioFileOpenURL` → `AudioAnalytics` → `CFBundleGetMainBundle` → `_CFIterateDirectory` → `readdir`。
起動直後だけの sample は `_dyld_start` しか示さない場合がある。`running 1 test` の後の処理を採り、起動待ちと本体の列挙を分ける。

ユーザーの許可、検査ロックの取得、Cargo / rustc の停止確認の後、`target/debug/deps` 直下の通常ファイル `*.rcgu.o` だけを削除した。
対象はリンク後に残った再生成可能なオブジェクトで、`incremental` 内や `.rlib` 内のオブジェクトまで広げない。
770,638 件（論理サイズ約 46.6 GB）を除き、直下は 6,300 件、ディスクの空きは約 6.9 → 22 GiB になった。
`.rlib`・`.rmeta`・`.d`・実行ファイル・動的ライブラリ・`.a`・鮮度情報・検査記録は残した。
比較したテストの SHA-256 は整理前後で一致し、同じ Cargo の `--no-run` は再コンパイルなしで既存成果物を再利用した。

比較には同一の CLI 回帰 `contract_offline_missing_plugin_exports_dry_audio_and_keeps_its_state` を使った。
整理前の後続 2 回が 12.63 / 4.94 秒、整理後 3 回が 0.25〜0.27 秒だった。
同じバイナリ・入力・期待値での環境切分けであり、製品の性能改善率や最悪時間の保証には使わない。
診断と整理の集計は `target/focused-checks/cache-cleanup.json`、比較と sample は `target/focused-checks/cache-startup-before/` に記録した。

オブジェクトは再ビルドで再び増えるので、今回の整理を恒久的な自動掃除と扱わない。
期限を延ばしたりテストを省略せず、整理後も通常の `make verify` で正式な結果を取り直す。

### `swift test` が出力なしで止まる（Swift Testing の待ち続け）

Swift Testing は始めた全テストの終わりを待つ。再開されない `await` が 1 つあると、全スレッドが休んだまま実行が続き、遅いだけに見える。
2026-10-25 にはこれで約 4 時間止まり、検査の 1,200 秒の期限の後に `swiftpm-testing-helper` が親を失って（PPID 1）残った。
健全な EchodeApp の全テストは 10〜15 秒である。手で全体を流す時は `perl -e 'alarm 180; exec @ARGV' swift test -q ...` のように上限を付ける。
`/usr/bin/perl` は SIP の保護下で `DYLD_*` を落とす。サニタイザの実行では上限の後に設定する
（`perl -e 'alarm 180; exec @ARGV' bash -c 'export DYLD_...; exec swift test ...'`）。そうしないと固定した TSan のランタイムが黙って外れる。
検査を止めた時と期限で終わった時は、再実行の前に `pgrep -fl swiftpm-testing-helper` で残りを探して止める。
止まったテストは、ログで始まって終わっていないものを並べて探す。必要なら `git stash` した基準と比べる。

### 空き容量不足（`No space left on device` / `Errno 28`）

`make verify` の事前確認 `disk-space` が止めた時も同じである（必要量は [検査手順](../verification.md#事前確認と実行順序)）。
2026-11-06 は事前確認後に空きが尽き、製品契約のログと `report.json.tmp` の保存も `Errno 28` で失敗した。
この場合、プロセスが終わっていても既存のレポートに `running` が残り得る。稼働の判定はロックと PID で行う。
実行ログ・容量・結果の更新時刻を保全し、`running` を合格に書き換えない。空き確保後に `make verify` で正式に記録し直す。
`df -h` で空き容量を確認する。検査のロック解除後に、再生成可能なコンパイラーキャッシュや一時データだけを整理し、既存の検査結果・ユーザー資料・開いているアプリは削除しない。空き確保後に `make doctor` と `make verify` を再実行する。
worktree ごとの `target` が大きい。Claude Code のデスクトップアプリでは、セッションのアーカイブで変更のない worktree が `target` ごと再利用の置き場へ移るので、アーカイブだけでは空かない（2026-10-31 に 25 GB）。
再利用できない生成物は消してよく、現在有効なキャッシュは空きが検査に足りない時だけ、再生成の時間を見積もって消し、報告する（[ADR 0276](../decisions/0276-priority-order-and-delegated-decisions.md)）。作業は `main` の作業ツリーにまとめ、使い終えた worktree の再生成できる `target` を消す。
最初の候補は、全体の段階の ASan・TSan・Miri が使う `target/bridge/address`・`thread`・`miri` である（2026-11-09 に合計 5.9 GB）。
GitHub の `full` がプッシュごとに実行する。作業ごとの `make verify` も、その対象の crate で `unsafe` を足し引きした時は選ぶ（`task_unsafe`）。
2026-11-09 は消した直後にこの条件に当たり、冷えた状態から ASan 432 秒・TSan 328 秒・Miri 75 秒かかった（温まった状態では 194・138・34 秒）。
`make verify-plan` がこの 3 つを選んでいる時は、消しても同じ実行で作り直すので空きは増えない。
その時の候補は `target/release` で、使うのは `spike-*` の release 計測だけである（Makefile の `--release`。2026-11-09 に 2.0 GB）。
`target/debug/incremental` は、同じ日の作業ではほぼ全部が使われていて、古い分は残らなかった。
2026-11-05 には空き 227 MiB の状態で、Swift が `internal error in Code Signing subsystem` だけを返して署名で止まった。
この表示でも `df -h` と、`swift build --build-tests` の署名の診断を採る。署名を無効にせず、容量の確保後に同じ経路で再実行する。
増分コンパイルのキャッシュだけを整理する場合も、検査ロックとコンパイラーの停止を確認する。
`target/debug/incremental` と、再利用する成果物・鮮度情報の `deps`・`build`・`.fingerprint` を区別し、後者と反例・サニタイザ専用のビルド先、検査記録は残す。
既に鮮度のある同じ Cargo ビルドを再実行し、成果物のサイズ・更新時刻が変わらないことを照合する。ソースの再ビルドが要る場合は遅くなるため、影響なしとは扱わない。

### `cargo test -p echode-plugin-host` を直接動かすと `hosted_playback` の多くが無音の断定で失敗する

`expected.iter().any(|sample| *sample != 0.0)` などで、プラグインのない描画まで無音になるなら、同梱の Sine Pad の `target/debug/plugins/EchodeSinePad.clap` がない。
`hosted_playback` の楽譜はこの音源で鳴らす。`make verify` は先に `make bundled-plugins` を実行するが、`cargo test` は作らない。
`make bundled-plugins` の後に再実行する（2026-11-11、ADR 0318 の作業で 10 件がこれで失敗し、作った後は 60 件が通った）。

### 検査の後に `target/debug/echode` のプラグイン処理が `control socket closed: dyld[...]: Library not loaded: @rpath/libclang_rt.rtsan_osx_dynamic.dylib` で失敗する

`make rtsan` が `target/debug/echode-plugin-host` を RTSan 版で上書きした状態である。製品の不具合ではない。
2026-11-02 から `make rtsan` は最後に通常版を作り直す。途中で失敗した時は `cargo build -q -p echode-plugin-host` で戻すか、
`ECHODE_PLUGIN_HOST` で別のビルドを指す（[外部監査の精査](../reviews/2026-11-02-external-audit-review.md)）。

## ツール・ビルド・生成物

### ツール不足・ビルド失敗

`doctor` の `missing_tools` と最初のエラーを読む。通常ツールは `make setup`（lint の固定版だけなら `make setup-lint`）、動的検査は `make setup-bridge`。
ツールがそろっていても、`setup_needed` に `setup-bridge` があれば TSan の検査専用ランタイムが未準備か、
Xcode・CMake・Ninja の版と合わない。この時 TSan の検査はソースの変更がなくても `stale` になる。
準備は毎回 CMake のビルドのディレクトリを作り直す。別の作業ツリーから写した `target/` のキャッシュは古い場所を指し、
以前は `CMakeCache.txt directory ... is different` で構成が止まった（2026-11-12）。
`swift test -q` が `Build failed` / `fatalError` だけを返す場合は、`swift build --build-tests` で診断を取得する。
同じ `--package-path`・`--scratch-path` と `-Xswiftc -warnings-as-errors` を指定し、`target/` のログで `error:` 周辺を読む。
Xcode 側のビルド診断が quiet 出力に出ない場合がある。大量のコンパイルコマンドは会話へ展開しない。
`-Zbuild-std --offline` が `failed to download addr2line` 等で止まる場合は、製品の依存とは別の固定 nightly の標準ライブラリ用キャッシュが不足している。
`make setup-bridge` の Miri sysroot 準備で取得してから検査を再実行する。ツールの存在確認だけでは、このキャッシュ不足は分からない。

### `check-ffi` が外部シンボルを拒否する

`scripts/check-ffi.sh` はファイルではなく、パスと関数・静的変数名の組を照合する。
`not allowed` は新しい宣言、`stale entries` は置換済みの許可行である。
`libc` / objc2 の公開宣言へ置き換え、古い行だけを除く。既存モジュールへの追記で回避しない。
`<link_name>` は許可済みの Rust 名の差替えを示す。通常のバインディングを使う（[ADR 0265](../decisions/0265-symbol-scoped-foreign-declarations.md)）。

### `check-audio-lints` が宣言や解除を拒否する

`scripts/check_audio_lints.py` は、音声経路の crate の lib のルートに 4 つの lint の拒否があることと、局所の `allow`・`expect` がないことを確かめる。
`does not deny` はルートの宣言が欠けたか lint が減った状態で、宣言を戻す。`allows clippy::...` は解除で、`get`・反復子・スライスのパターンへ書き換える。
音声スレッドで動く crate を足した時は、スクリプトの `AUDIO_CRATES` に足す。spike 専用の `graph.rs` の解除だけは許す（[ADR 0284](../decisions/0284-audio-path-panic-lints.md)）。

### clippy が `unsafe` の根拠を求める

`undocumented_unsafe_blocks` は `forbid` で、`unsafe` のブロックと `impl` の直前に `// SAFETY:` で根拠を書く（[ADR 0321](../decisions/0321-unsafe-safety-comments-and-ffi-arguments.md)）。
`allow(...) incompatible with previous forbid` は局所の解除で、許されない。根拠を書けない時は、呼出し側の契約を `unsafe fn` の `# Safety` に移すか、コードを直す。
複数行の式の中のブロックは、式の前の注釈では足りないことがある。ブロックの行の直前へ移すか、ブロックを `let` に分ける。
`unnecessary_safety_comment` は、`unsafe` のなくなった所に残った根拠である。消すか、根拠の要る行へ移す。
C ABI の入口で呼出し側のメモリを読み書きする時は、直接ではなく `crates/echode-ffi/src/abi.rs` の関数を使う。

### `check-workspace-lints` が lint の継承を求める

`scripts/check_workspace_lints.py` は、`[workspace] members` の全メンバーの `Cargo.toml` に `[lints] workspace = true` があることと、
ワークスペースの `undocumented_unsafe_blocks` が `forbid`、`unsafe_op_in_unsafe_fn` が `deny` 以上であることを確かめる。
`does not inherit the workspace lints` は新しい crate の宣言漏れで、`[lints]` に `workspace = true` を書く。

### `check-typed-errors` が文字列のエラーを拒否する

`scripts/check_typed_errors.py` は、`Cargo.toml` の `[workspace] members` の全メンバーの `src` を調べる。
一覧はなく、新しい crate はワークスペースへ足した時から対象になる（[ADR 0320](../decisions/0320-typed-package-entry-errors-and-workspace-check.md)）。
`Cargo.toml: missing`・`no workspace members`・`<member>/src: missing` は、マニフェストかメンバーのソースが読めない状態である。
`*_tests.rs`、`tests/`、末尾の `#[cfg(test)] mod tests` は対象外である。

- `returns a String error`: `Result<_, String>`。署名・チャネル・クロージャの型を、複数行と入れ子も含めて見る。
- `turns an error into text`: `map_err` の中で受け取ったエラーを `.to_string()`・`format!` で文にした所。元のエラーを変種に持たせ、文は `Display` で作る。
- `wraps error text in an I/O error`: `io::Error::other`・`io::Error::new` に `format!`・`.to_string()` を渡した所。`map_err` の外も含め、種類を問わない（[ADR 0311](../decisions/0311-typed-shared-memory-errors.md)、ADR 0320）。
- `gives an I/O error only constant text`: I/O エラーに文字列リテラルだけを渡した所。種類が原因そのものの `TimedOut`・`UnexpectedEof`・`WriteZero` は許す（[ADR 0317](../decisions/0317-typed-control-transport-errors.md)）。
- `returns an untyped package message`: `echode-history` の `PackageError::new`・`Message` の再導入。

I/O エラーの原因は、内側の型にして `io::Error::new(kind, 型)` で包む（`DescriptorRuleError`、`PackageEntryError` など）。種類と表示文は保ち、呼出し側は `get_ref` で読む。
拒否されたら、その crate のエラー型（`echode-audio-io` は `AudioError`、`echode-document` は `DocumentError`、`echode-sandbox` は `SessionError`・`WorkgroupError`・`SpawnError`、ヘルパーの拒否は `Refusal`、プラグイン自身の失敗は `PluginFailure`、`echode-engine` は `PlaybackError`・`DeviceError`、`echode-plugin-host` は `HostError`・`ScanError`・`CatalogError`・`WatchError`）の変種を使うか、原因に合う変種を足す。
`Clone` でない原因（`SessionError`・`io::Error` など）は `Arc` で包む。文字列にするのは、アプリ・API・ログへ渡す報告の欄だけにする（[ADR 0291](../decisions/0291-typed-audio-io-errors.md)〜[ADR 0295](../decisions/0295-typed-plugin-host-errors.md)）。
`echode-ffi` の目録は `catalog::CatalogError` を返す。元の目録・JSON の原因を保持し、C ABI の失敗の数値へ畳むまで文にしない（[ADR 0302](../decisions/0302-typed-ffi-catalog-errors.md)）。
`echode-model` の `ModelError::Invariant` の内側は `InvariantError`、`Order` の内側は `ArrangeError` で、原因を文へ戻さない（[ADR 0303](../decisions/0303-typed-model-invariant-errors.md)）。
`echode-api` は `ApiError`・`PerformError`・`ProtocolMessage` に元の I/O・保存・JSON・文書の原因を保持し、wire 応答を作る直前まで文にしない（[ADR 0304](../decisions/0304-typed-api-errors.md)）。

`echode-history` は非既定の履歴スパイクも含めて対象である（ADR 0296〜[0301](../decisions/0301-typed-history-spike-errors.md)）。
`Io`・`Audio(MediaError)`・`Git`・`Validation(PackageValidationError)` など原因の変種を使う。
スパイクの JSON・版・ワーカーの失敗だけは feature 付きの `Spike(SpikeError)` に置く。
複数の待ち手へ渡す失敗は `Shared` で原因を共有し、文を複製しない。`original()` で元の変種を調べる。
検査は構文で見るので、変数に入れた `&'static str` や型の推論を要する経路は見つけない。

### `check-lint` が整形の差や lint の指摘を報告する

- `lint tools: ... run make setup-lint`: 固定版が未導入か、`scripts/lint-tools.txt` が導入後に変わった。`target/` を消した時も起きる。導入はネットワークを使う。
- `Would reformat`（ruff）、`--list` のファイル名（shfmt）、`[Indentation]` などの配置（swift-format）: `make fmt` で直す。
- 規則の名前付きの指摘: コードで直す（[ADR 0323](../decisions/0323-swift-python-shell-format-and-lint.md) の「直した指摘」が例）。抑止は理由を書いた時だけ使う。
- コードを変えていないのに swift-format の差が出る: Xcode の更新で swift-format が変わった。`make fmt` の結果だけを 1 つのコミットにし、`.git-blame-ignore-revs` に足す。
- `make fmt` の後にビルドが通らない: swift-format が括弧を外した比較 `a < b, c > (d)` は、次の整形で `a<b, c>(d)` というジェネリクスの呼出しに読み直される。
  `make fmt` は swift-format を 2 回かけるので、この形はビルドで止まる。比較の順を変えるか、値を先に変数へ入れる。

### Cargo の依存メタデータが欠ける

`extern location for <crate> does not exist` で `target/debug/deps/*.rmeta` が欠ける時は、まず別のビルドや整理処理との競合を確認する。
製品の型エラーと区別し、処理がない状態で欠落した依存だけを `cargo clean -p <package>` して再生成する。
検査結果やユーザー資料は削除しない。2026-10-29 の波形キャッシュ検査では `memchr`・`errno`・`bitflags` で観測した。
作業時の記録は `target/focused-checks/peak-cache-green/` に残し、正式な合否は `make verify` で確認する。

### Swift の編集型の生成鮮度が不一致になる

`make check-swift-actions` の `is stale; run make swift-actions` は、製品の編集拒否ではなく登録簿と生成物の差である。
`make swift-actions` で再生成し、`EditAction.swift` の差分と `EditActionTests` の共有 fixture を照合する。
未対応のスキーマや名前の衝突は生成器のエラーとして拒否する。生成物を手で直して回避しない。
`typed-edit-bridge` は文書ワーカー・履歴・再オープンを検査するが、controller の辞書の移行完了の証拠ではない
（[ADR 0202](../decisions/0202-generated-swift-edit-actions.md)）。
ピアノロールは `typed-piano-controller` と既存の操作契約を使う。
型付きのノート要求が拒否される場合は、`performActions` の前の幾何・MIDI 範囲・元の値と、`applyEdit` の文書照合を分ける。
MIDI の `UInt8` 変換は検証後だけで行う。非整数化や clamping で拒否を隠さない。
旧文書・古い元の値・モデルの拒否を区別する。
トラックとデバイスは `typed-track-device-controller` と既存の操作契約を使う。
追加した根の選択は `performActions` の `selectingAddedTrack`、順序と no-op は `applyEdit` の手番の前後で調べる。
並べ替え・コピーの `before: null` は末尾指定であり、ピンの `UInt32.max` は CLAP の予約 ID としてモデルが拒否する。
ミキサーと音声クリップの値は `typed-mixer-controller` と既存の入力・文書境界の契約を使う。
試聴の確定だけが失敗する場合は `performActions` から `applyEdit` へ渡す `finishingPreview` と、終了時の文書照合を調べる。
フェードは整数 flick を保持するが、モデルの `FADE_RATE_LIMIT` で `u32` フレームに収まらない値は従来どおり拒否する。
クリップ移動・複製と音声取込みは `typed-clip-controller` と既存の実入力の契約を使う。
tick の精度・捕捉したトラックと、移動後の選択・複製側のフォーカスを分けて調べる。
`action reference is out of range` や `action reference field is missing` は、`EditText.result` の番号や項目名の誤りである。
番号は同じ配列の 0 からの位置で、項目は確定後のアクションの文字列を指す。失敗すると一括編集全体が巻き戻る。

### Rust に追加したアクションを Swift が `unknown variant` として拒否する

Swift のテストは Rust の製品 ABI をリンクするため、先に `make check-shell` で ABI と生成ヘッダを更新して再リンクする。
Swift の単体実行だけでは古いアクション登録簿が残り得る。更新後も失敗する場合は、送信した `type` と登録簿を照合する。
`echode_model_constants` の未解決シンボルや単位の不一致も、同じ製品 ABI・生成ヘッダ・再リンクから調べる。
`ModelLimitsTests` と `ModelUnitsTests` の独立した期待値で、橋渡しと UI 消費側のどちらが違うかを区別する。

### macOS 27 / M4 ビルド基準の不一致

Cargo 生成物は `xcrun vtool -show-build <バイナリ>` と `lipo -archs` で確認する（単体の `rustc --print deployment-target` は `.cargo/config.toml` を読まない）。
Swift は `-target-cpu apple-m4` と `-Xcc -mcpu=apple-m4` を対で指定する（[ADR 0114](../decisions/0114-m4-build-baseline.md)）。
release の proc-macro / cdylib 読込失敗（`E0463` / `mis-aligned LINKEDIT string pool`）は `.cargo/config.toml` の `strip = "none"` を確認する（[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)）。
`scripts/check_build_host.py` が `sysctl machdep.cpu.brand_string` を読めない隔離環境では、CPU 情報の読取が許される環境で再実行する。

### GitHub Actions の失敗

`check` は `make check-portable` と `make advisories`、`full` は全体の段階を実行する（[ADR 0269](../decisions/0269-github-public-ci-scope.md)）。
ログは `gh run view <実行ID> -R crab-hoo/echode --log-failed` で読む。
`full` の失敗は、同じ make の入口を手元で実行して再現する。反例の 1 シャードは `ECHODE_CONTRACT_SHARD=<I>/4 make check-contract-controls`。

- `toolchain '<版>-aarch64-apple-darwin' is not installed`: ランナーの rustup は `rust-toolchain.toml` の固定版を自動では入れない。
  手元の rustup は既定で自動導入するので、そのままでは再現しない。
  `RUSTUP_HOME=<空のディレクトリ> RUSTUP_AUTO_INSTALL=0 make advisories` で同じ失敗になる。
  cargo を呼ぶジョブは、その前に `rustup toolchain install` を実行する。
- 空の `RUSTUP_HOME` で `rustup toolchain install` を実行すると、rustup 本体の自己更新も走る。
  先に `RUSTUP_HOME=<同じディレクトリ> rustup set auto-self-update disable` を実行する。
- `test_a_temporary_identity_signs_the_helper_app_and_leaves_the_main_app_strict` の skip `the runner does not enforce library validation`:
  hardened runtime の対照がアドホック署名のライブラリを読み込めた。CI のホストの性質で、署名の誤りではない。
  手元の Mac で同じ文言が失敗として出たら、そのマシンがライブラリ検証を強制していない（`csrutil status` を確認する）。
  対照が拒否されたのに主アプリが読み込んだ時は、署名か entitlement の誤りで、`dlerror()` の出力が stderr に出る。
- `Native verification requires macOS 27+ on M4 or later; got ..., Apple M1 (Virtual)`: ホスト型ランナーに M4 はない。
  作業の段階のネイティブ検査を CI へ戻さず、手元の `make verify` で行う。`full` の make の入口はこの判定を呼ばない。
- `full` のサニタイザで `Export CLI is missing from the app bundle`: テストが起動する CLI などが `target/debug` にない。
  入口の `make sanitizer-prerequisites` が先に動いたかを確かめる。手元では前の検査の残りで隠れる。
- `full` の `Illegal instruction`（終了コード 132、シグナル 4）: apple-m4 のビルドに、M1 にない命令が入った。製品の不具合ではない。
  `xcrun llvm-objdump -d --mcpu=apple-m1 <バイナリ>` の `<unknown>` が、`--mcpu=apple-m4` より多いかで確かめる。
  増えていたら、CI だけ apple-m1 でビルドする切替えを足す（ADR 0269 の追記で見送った案）。反例はこの落ち方を検出と数えない。

### Dependabot のアラートがあるのに `make advisories` が合格する

`make advisories` は RustSec の DB だけを照会し、Dependabot は GitHub Advisory Database を照会する。
GitHub だけにある勧告では、この 2 つが食い違う（[ADR 0270](../decisions/0270-gix-0-90-and-github-advisories.md)）。

- アラートは `gh api 'repos/crab-hoo/echode/dependabot/alerts?state=open'` で一覧する。
  `--jq '.[] | {n: .number, ghsa: .security_advisory.ghsa_id, pkg: .dependency.package.name, patched: .security_vulnerability.first_patched_version.identifier}'` で要点だけを読む。
- RustSec に載っているかは、`~/.cargo/advisory-dbs/*/crates/<crate>/` の `aliases` を GHSA の ID で `grep` する。
  無ければ、cargo-deny が合格するのは正しい挙動で、DB の取得の失敗ではない。
- 修正版は `cargo info <crate>@<版>` と crates.io の依存要求で、上位の crate のどの版が修正版を引くかを調べる。
  更新後の lockfile は `gh api -X GET advisories -f ecosystem=rust -f affects='<crate>@<版>,...'` で照会し直す。
- 各勧告が Echode から到達するかは、勧告が挙げる API を Echode が呼ぶかで判断し、その理由を ADR に残す。

### 実行環境の権限制限（共有メモリ・キーチェーン）

`shm_open: Operation not permitted` や `SecKeychainCreate` の失敗はサンドボックス制限によるため、許可された環境で再実行する。
Swift のマニフェスト読込時に `ModuleCache: Operation not permitted` と標準ライブラリ読込失敗が続く場合も、先にキャッシュの書込権限を確認する。
文書だけの検査でもビルド設定と一時署名の検査を含むため、これらの環境エラーを製品の型エラーや設計資料の不整合と混同しない。

## 検査の結果と記録

### 長時間検査の前の準備不足

`make verify` の `事前確認 ... failed` は製品テストをまだ実行していない環境エラーである。
`target/verification/preflight-<ID>.log`、`report.md` の復旧案内を確認する。
`make debug-report` は登録済みの事前確認ログと失敗注入の計測 JSON も収集する。
任意の個別コマンドの出力は私的な内容を含み得るため、自動収集せず必要なログを選んで添付する。
`tsan-runtime` は既存の準備確認で、古い文脈・マニフェスト不正・両 dylib の変更を拒否する。復旧は `make setup-bridge`。
`bridge-nightly` / `bridge-miri` は固定 nightly・rust-src・Miri、`cargo-offline` はローカルの依存不足を切り分ける。
事前確認を飛ばして合格にせず、復旧後に同じ `make verify` を再実行する。

### `spike-scale` が結果の書込みで `No such file or directory` になる

外部の `CARGO_TARGET_DIR` を使った新しい worktree では、コンパイル先だけが作られ、作業ツリー内の `target/` がない場合がある。
`scale.rs` の結果の書込みが失敗したら、モデルや再生の失敗と区別し、その worktree で `mkdir -p target` を行って同じ計測を再実行する。
結果はコンパイル先ではなく、その worktree の `target/spike-scale.json` に書く。途中の出力だけを計測の合格にしない。

### コマンドは成功扱いなのにログに失敗がある

`echo`、`;`、`grep` / `head` のパイプによる終了コードの置換を調べる。
[個別コマンドの記録](../verification.md#作業中の個別コマンド) の `commands.json` と完全ログを使い、成功表示だけを証拠にしない。
macOS 標準 make 3.81 は `.SHELLFLAGS` を無視する。厳密なレシピ実行は Makefile の `SHELL` のオプションで行う。

### `全体検査で未確認` の表示

失敗ではなく、現在の入力で手元に全体の段階の合格の記録がないことを示す。CI の `full` の合格はここに出ない。
対象のコミットの `full` の結果は `gh run list -R crab-hoo/echode --workflow full --commit <SHA>` で見る。
`full` は `main` へのプッシュで動くので、プッシュ前の不具合はそれまで検出されない。手元での発生時期は `timing-history.jsonl` と `report.json` の `finished_at` で絞る。

### `negative control no longer matches`

製品コードの編集で、反例の置換元が現在のソースに 1 回だけ現れなくなった。ビルドの前に全反例で確認する。
検査を緩めず、`scripts/verification.json` の `old` / `new` を本来の不具合を戻す差分へ更新する。

### `the unmutated source copy does not build`

反例を入れる前のコピーがビルドできない。反例の検出とは無関係で、コピーの同期か本体のビルドを調べる。

### 自動検査・製品契約の失敗

`make verify` 失敗時は標準エラーに出力される失敗箇所の抜粋を確認し、必要なら `target/verification/<ID>.log` または `product-contracts/` の該当ログを読む。
最適化等で `negative_control` が期待値で落ちなくなった場合は、本来の不具合を再現する差分へ更新する（例: `loop-head-residency`）。
Swift の反例ビルドは Rust ABI を作り直さない。Rust の差分を Swift 契約へ直接登録すると、
未変更の ABI へリンクして反例を検出できない。モデルの反例は Rust 契約、UI の反例は Swift 契約に分ける。
`check-product-contracts`・`check-contract-controls` 全体の `VERIFICATION TIMEOUT` と、個別契約のタイムアウトを区別する。
前者では `product-contracts.json` の各記録の `seconds` と `reused` で進んでいた契約を確認する。
`reused: true` は同じ識別値で前回合格した記録であり、それ以外の古いログを今回の完了証拠にしない。
作業の段階の通常テストは、バイナリごとのまとめた実行の記録（`isolation: "batch"`、`batch-*.log`）と単独の記録が混ざる。
まとめた実行で示せなかったテストは単独で実行し直すので、失敗の調査には単独のログ（`<契約ID>.log`）を読む。
まとめて落ち、単独で通るテストは、並列の負荷か他のテストとの干渉を疑う。全体の段階は単独の実行だけを使う。
`missing, skipped, or failed required test` なのに Swift が成功を表示する場合は、指定した関数名とケース数の表示を確認する。
引数付きの関数は `with N test cases`（N は 1 以上）まで照合する。引数なしだけを認識していた判定は 2026-11-05 に修正した。
正例・反例とも同じ表示形式を認識し、0 ケース・スキップ・別関数・複数の関数・ビルド失敗を合格や不具合の検出に数えない。
一括予算は `scripts/verification.json` の各検査の `timeout`、各契約の期限は別の値である。
Swift のコピーの初回ビルドは `SWIFT_COPY_BUILD_TIMEOUT` の範囲で行い、契約の期限に算入しない。
2026-10-29 に旧 1800 秒で全件失効時の上限到達を観測し、件数と実測に合わせて一括予算を見直した（[ADR 0195](../decisions/0195-verification-cost-observability.md)）。
反例のコピーは `target/product-contract-mutations/checkouts/<N>` に残り、`<N>.json` が置いたファイルの一覧である。
一覧が壊れていればコピーのソースを作り直す。コピーを疑う時は `checkouts/` を消せば次回に作り直す（初回ビルドの分だけ遅い）。
Rust の反例のビルドは `target/product-contract-mutations/cargo` に置き、この検査以外から書き込まない。
別の書き手の時刻が混じると、反例で作ったクレートを現在のソースのビルドと誤認しうる（[ADR 0148](../decisions/0148-verification-pipeline-speed.md)）。
別の実行が `target/product-contract-mutations/.lock` を持つ時は 30 秒で失敗する。`make verify` を並行して動かさない。
`background-graph` の初期化待ちが 5 秒未満で `WouldBlock` を返す時は、accept 後の `O_NONBLOCK` を調べる。
Darwin は非ブロッキングのリスナーのフラグを受け継ぐため、読込み期限の設定だけでは待機に戻らない。
検査用の `initializer_peer` と、送信前のフラグを確認する通常回帰を使う（[ADR 0142](../decisions/0142-background-plugin-graph-preparation.md)）。
2026-10-30 の編集型生成の検査では、`mcp-backend-framing` の正常回帰が 5 秒の EOF 待ちで一度失敗した。
`handshake=true`・改行なし・`following=""`、Darwin errno 35 であり、反例の検出とは区別する。
当初は同じ Cargo 構成の単独実行では再現せず、原因は未確定だった。この時点では MCP の製品コード・期限・回帰は変更していない。
初回の失敗と単独再実行は `target/focused-checks/typed-edits-mcp-framing/` に残す。
再発時は接続の所有権・半閉鎖後の読取り・実行中のプロセスの状態を採取し、正式な `make verify` と区別する。
後続のピアノロール移行で再発し、fixture の `Value::Display` の直接書込みでは反復して再現した。
製品と同じ先行文字列化と `write_all` へ変えると反復では再現しなかった。OS 内部の原因は未確定である。
送信内容・半切断・期限・アサーションは維持し、サーバーとクライアントの時間・出力を失敗時に残す
（[ADR 0115 の fixture 補強](../decisions/0115-phase2-mcp.md#半切断-fixture-の送信と診断の補強)）。
同じ作業の次の実行では、通常回帰後・Rust 反例 95 件の完了後、最初の Swift 反例ビルド群が個別期限に達した。
全体は約 954 秒で、3600 秒の一括予算到達ではない。群の強制終了前のコンパイラー出力は保存されていない。
`ArrangeAudioDrop.swift` と `ArrangeClipMove.swift` の 3 コピーが対象で、残留プロセスはなかった。
中断時点は同じ作業ログの `counterexample-timeout.log` に記録し、対象や期限は変えず未完了分を再実行する。
2026-10-31 に新しい worktree で同じ位置（Rust 反例 101 件の後）の停止を再現した。コピーに Swift のビルドがなく、最初の群が 3 コピー同時の初回ビルドになり、180 秒の個別期限に達する。
完了した反例の記録と途中までのビルドは残るので、再実行で先へ進む。追加のビルドは 1 コピー約 25 秒だった。製品やテストの失敗と区別する。
新しい worktree ではこの検査だけで数 GB を超え、Rust 反例の `cargo` は一時 26 GB に達した。空きが少ない時は、記録済みの反例の `cargo` を消して再実行できる。
新しい worktree の初回は、別の作業ツリーの `target/debug`・`target/product-contract-mutations/cargo`・`target/toolchains` を APFS の複製（`cp -c -R -p`）で写すと、ソースに触れずに依存のビルドを使い回せる。
2026-11-01 に `target` を消した main でも、Rust 反例 103 件の後に同じ停止が起きた。
この時は親を失った `swift-frontend` がコピーの中で 20 個以上動き続け、負荷平均が 50 を超えた。
各プロセスが自分のプロセスグループで動くので、検査が止めるグループの外に残る。
再実行の前に `ps -Ao pid,ppid,comm` で親が 1 の `swift-frontend` を探し、`lsof -a -p PID -d cwd` で作業フォルダーがコピーの中と確かめて止める。
`negative control must stay inside its source copy` は、期待値の不一致ではなく反例用コピーの検証エラーである。
対象ファイルの欠落と、シンボリックリンクによるコピー外への逸脱を区別する。
`target/` の整理と検査が競合すると、コピーが実行中に消えて同じエラーになる。
整理の完了と検査のロック解除を確認してから再実行し、コピーの欠落を製品不具合の検出と扱わない。
コピー内のディレクトリ削除時の `.DS_Store` 競合は自動リトライされるが、権限エラー等の継続失敗は合格にしない。
履歴の `roundtrip` が `requires the features: spike` で拒否された場合は、非既定 feature の指定漏れである。
`cargo test -q -p echode-history --features spike --lib --test roundtrip` を使う。計測は `make spike-history` を使う。
製品のテストを通すために workspace 全体でこの feature を有効にしない（[ADR 0211](../decisions/0211-opt-in-history-spike.md)）。
エンジンの `echode-engine-spike` が同じ理由で拒否された場合も、`--features spike` の指定漏れである。
通常回帰は `cargo test -q -p echode-engine --features spike --lib`、実機計測は明示的な依頼がある時だけ `make spike-engine-smoke`・`make spike-engine` を使う。
製品の `playback`・機器一覧・Workgroup の問い合わせのビルドに、この feature は不要である
（[ADR 0213](../decisions/0213-opt-in-engine-spike.md)）。
単独では通る保存・復旧の回帰が、復旧後のトラック数などの過剰で落ちる時は、`$TMPDIR` に残った同じ pid の一時パッケージを疑う。
失敗した過去の実行が残したジャーナルが再生され、2026-10-21 に `phase1-acceptance` で再現した。
一時パッケージを作る `scratch()` は使用前に同じパスを削除する。

### ASan / TSan の Swift 検査で `Status did not arrive`（復旧の警告待ちの期限切れを含む）

原因は主アクターの占有で、応答の喪失でもデータ競合でもない。
Swift Testing は `@MainActor` のテストを同時に開始し、全テストが主スレッドの 1 本の列を交互に使う。
あるテストが `await` なしで長く処理すると、他のテストのポーリングも応答の受取りも止まり、実時間の 5 秒だけが減る。
目印はポーリングの回数である。喪失なら 5 秒で約 1000 回進むが、占有では `1 yielded polls` のように数回で切れる。
単独実行で再現しないのは、占有する相手がいないためである。
2026-10-31 に特定した占有の主は、全画素を主アクターで走査していた次の 2 テストだった。
`CanvasSubmissionTests.contractStaleDrawableSubmissionDrawsAndRetries` と `ArrangeTests.contractMismatchedDrawableKeepsTopLeftPixels`。
連続する占有は TSan で 4.0〜6.0 秒、ASan で 3.5〜4.6 秒に達し、どの実行でも 5 秒の窓をほぼ使い切っていた。
待ちの開始が占有の直前に重なった時だけ失敗し、修正前の TSan 全体では 21 回中 6 回失敗した。
`wait` の途中で主アクターを 5.4 秒止める一時変更でも、同じメッセージで 3 回中 3 回失敗した。
全面走査を `CanvasReadback.scan` で主アクターの外へ移した。比較の内容と 5 秒の期限は変えていない。
通常の全体実行では解消した。修正後は TSan 全体 50 回、ASan 全体 26 回の完走で、この期限切れは 0 回だった。
TSan の同じ待ちの所要は 1.4〜2.9 秒になった。修正前は通常でも 7.6 秒に達した。
全コアに負荷をかけた TSan 全体では減ったが、なくならなかった。修正なしは 20 回中 5 回、修正ありは 29 回中 1 回失敗した。
修正ありの負荷下の待ちは 2.2〜5.1 秒まで伸び、余裕はほぼなかった（2026-10-31 の main の上で 8 回測定）。
失敗した 1 回は全体の所要が 79.8 秒（他は 37〜76 秒）で、28.2〜33.5 秒の停止の直後に期限が切れた。
この停止では監視スレッド自身も 3 標本しか取れず（通常は 1 秒に約 3.5 標本）、特定のテストの占有より CPU の飽和とみられる。
同じ実行では `MixerStepTests` と `MixerValueBatchTests` も同じ 5 秒の待ちで失敗した。こちらは `undoCount` の不一致として出る。
修正後の最長の停止は ASan で 1.6〜3.0 秒、TSan は 2.5〜6.7 秒である。
3.1 秒を超える回は 2026-10-31 の main（5f03b80）の上でだけ見えた。差の原因は調べていない。
長い停止は実行直後の AppKit の遅延読込みと初回のビュー構築で、入力欄のフォーカスや `NSMenuItem` の実行を契機に数百のフレームワークが主スレッドで読み込まれる。
長い停止の標本には複数のテストが並ぶので、1 つのテストの占有とは限らない。TSan の負荷なしで 6.7 秒の停止があった回も合格したので、停止の長さだけで判断せず標本の主を見る。
`DYLD_PRINT_LIBRARIES` の出力は原因ではない。外しても同じように失敗し、占有の長さも変わらなかった。
負荷の下で残った期限切れの主因は全テストの同時実行で、2026-11-01 に合否を決める実行の同時実行数を 16 に絞った（[ADR 0210](../decisions/0210-sanitizer-test-width.md)）。
絞らないと約 260 個のテストが同時に生き、`await` やポーリングの 1 回ごとに全テストの一巡を待つ。
飽和の下の主スレッドの時間は、ファイル削除が約 3 割、フレームワークの読込みが 3 割弱、開閉ボタンの入れ子のランループが約 5% を占めた。
同じ負荷（スピナー 10 本、各 30 回）で、`the unheld device parameter reply` の待ちの中央値は 2.64 秒から 0.01 秒になった。
任意の 5 秒の中の停止の合計は、全回 4.5 秒以上から全回 2.67 秒以下になった。
失敗の有無は、この負荷では前後とも 30 回中 0 回で差が出ない。
3 倍の負荷（スピナー 30 本、各 10 回）では 10 回中 6 回から 10 回中 0 回になった。
ただし、その負荷の後でも任意の 5 秒の中の停止の合計は 3.9〜4.85 秒に達する。
全テスト同時の実行は TSan の競合を探すために残し、テストの失敗を判定しない。
`race-swift-tests.failed.log` のテストの失敗は、サニタイザの報告がなければこの検査の失敗ではない。
`check_test_width: N tests ran at once, above 16` は、実行した本体の幅が上限を超えたことを示す。ADR 0210 の前提とツールチェーンを調べる。
引数付きテストは親の関数ではなく `testCaseStarted` / `testCaseEnded` を数える。
2026-11-05 に親 6 件と通常の本体 16 件を足した誤検出を修正した。同じイベント列の実際の本体の最大は 16 件だった。
`at most N tests at once ... went unchecked for races` は、競合を探す実行が絞られたことを示す。呼出し元の環境やツールチェーンの変更を疑う。
残る候補は、システム全体の 5 秒を超える停止である。全テスト同時で 124 秒、幅 16 で 1 テスト 116 秒の実行が、`mdworker` などの動く約 5 分の間に 1 回ずつあった。
監視スレッドも止まるのが目印で（`watcher gap`）、原因は確かめていない。この種類は同時実行数を絞っても吸収できない。
再発時の手順:
1. `target/bridge/<sanitizer>/product/swift-tests.failed.log` のポーリング回数で、占有と応答の喪失を区別する。
2. 製品の検査でビルドした後、製品の検査と同じ幅で占有の主を特定する（ASan は `address`）。
   `SWT_EXPERIMENTAL_MAXIMUM_PARALLELIZATION_WIDTH=16 python3 -B scripts/main_thread_stalls.py thread --runs 5`
   環境変数がないと全テスト同時になる。実際の幅は報告の `at most N tests at once` で確かめる。
   期限切れの時刻と直前の停止、テストごとの占有時間を `product/main-thread-stalls.log` に残す。判定には使わない。
   計測用に手でビルドし直す時は、製品の検査と同じ `-DECHODE_TSAN` を付ける。抜けると誤検出で止まり、報告の出力で停止も伸びるので、計測にならない。
   別の作業と重ねた実行で出た時は、特定のテストの占有か、CPU の飽和かをここで区別する。
   `longest stall load` の主スレッドの CPU が停止の長さに近ければ占有、`runnable` が大きければ CPU 待ち、`watcher gap` も大きければプロセス全体の停止である。
   `--profile` は主スレッドを 50 ms ごとに標本化し、読込み・削除・開閉ボタン・テストの処理に分ける。主スレッドを止めるので停止は伸びる。
   `/usr/bin/sample` やスレッド監視も対象を止める。スレッドが多いと影響が大きいので、比較の計測には残さない。
3. 占有するテストの同期処理を `await` で区切るか、主アクターの外へ移す。
期限の延長、検査の無効化、対象の除外で合格にしない。
復旧の警告待ちの期限切れ（2026-10-28）も同じ占有が原因とみられるが、再現はしていない。
`RecoveryConsentTests` の別プロセス実行はそのまま残し、失敗は各 `product/recovery-swift-tests.failed.log` に残る。

### Swift のミキサー回帰で `Expectation failed: overlapped`

旧 `repeatedMixerStepsLandWhileTheFirstEditIsStillBusy` は `Task.yield()` 1000 回の間に編集の `busy` を観測しようとしていた。
編集が主アクターへ制御を返す前に完了すると、値と Undo が正しくても観測だけを取り逃す。2026-11-05 の ASan 全体で再現した。
現行の `repeatedMixerStepsQueueWhileAnEditTurnIsBusy` は `TurnGate` で実際の `EditTurn` を保持し、UI と同じ `arrangeMixActions.adjust` を 2 回送る。
保持中にモデルと Undo が変わらないこと、解放後に両操作が成功して二段進み、Undo が 2 件増えることを確かめる。
回数や期限を増やして偶然の重なりを待たず、同じ不具合はゲートで順序を固定して検査する。

### Swift のミキサー回帰でアクセシビリティ操作の値と Undo の待機が外れる

`contractAccessibleStepsPreserveCapturedGroupOffsetsAndHistory` は、連続した音量の増分が 2 件とも反映するまで 5 秒待つ。
2026-11-10 の `check-swift` で、その直後の Undo 数と 2 トラックの音量だけが一度失敗した。
後続のパン・Undo 数の照合は失敗しておらず、単独実行（0.306 秒）と同じ全体実行の再実行では再現しなかった。
原因は未確定で、入力の消失・負荷による反映の遅れ・主アクターの待機を、この結果だけで区別しない。
初回の抜粋は `target/verification-control-text-retry.log`、単独の記録は `target/focused-checks/control-text-mixer/` にある。
再発時は、待機終了時の Undo 数・両トラックの値・busy と編集の完了時刻を採り、後続の操作へ進む前の状態を比較する。
期限を延長して隠したり、単独の成功で全体の失敗を見送ったりせず、正式な `make verify` を再実行する。

### 中断後に `running` が残る・全結果が一度に `stale` になる

呼出し元の時間切れは検査自身の終了コードではない。ロックが空いていても、自分が起動した make / cargo / テストの子が残る場合がある。
`ps` と保存ログで所有関係と終了を確認し、残った検査と次の検査を重ねない。既存アプリや他の作業のプロセスは終了しない。
子のログが後から成功しても、親が完了を記録していなければ合格へ手で移し替えず、`make verify` を再実行する。
検査の OS 識別はホスト名を除外するため、ネットワークによる名前変更だけでは失効しない。
OS・カーネル・アーキテクチャやツールの変更は引き続き失効させる（[ADR 0195](../decisions/0195-verification-cost-observability.md)）。
2026-10-28 の ADV-04 検査では、旧実装のホスト名による全件失効と中断の組合せを観測した。
当時のログと、現在の入力に結び付いた有効な検査記録を区別する。
Swift の負の対照を同じ位置まで何度もやり直す場合は、契約ごとの完了記録が増えているかも確認する。
各契約の全反例が終わった時点で保存する。一部の反例だけが終わった契約は合格ではない（[ADR 0148](../decisions/0148-verification-pipeline-speed.md)）。

### 負の対照の `NOT rejected` / `Build failed`

反例は、ビルドが通り、テストの `Expectation failed` で落ちた時だけ拒否と数える。
コンパイルエラーと `Fatal error` のクラッシュは、アサーションが欠陥を捕まえた証拠にならない。
逃げるクロージャ内の `self.` の欠落や、境界値での `Int64(Double(...))` のトラップを `<契約>-negative.log` で確認し、反例側を直す。

### 基準コミット（`VERIFY_BASE`）エラー

一時リポジトリのテストでは親の `VERIFY_BASE` を隔離する。無効な ref 指定は HEAD へ黙って置換せずエラーにする。

### 検査の直後の不要な再実行

ツール識別は PATH 文字列ではなく解決された `rustc` / `swift` 等の実パスを使う。

## サニタイザー

### TSan の既知の環境要因と製品不具合の切り分け

1. UI テストは機器なしの応答差し替え口を使い、実 HAL を開かない（`target/macos27-audit/tsan-hal-before.log`）。
   `EchodeAppTests` のバンドルでは `RealHALGuard.swift` が HAL の関数をリンク時に置き換え、機器を開かずに失敗させる（[ADR 0209](../decisions/0209-real-hal-guard.md)）。
   `Real HAL reached:` で全体実行が失敗し、Swift Testing がテストを「unknown」と示したら、`make check-real-hal-names` がテスト名を示す。
   `check-swift` は、テストが合格していてもこの報告があれば止まる（[ADR 0271](../decisions/0271-task-tier-duplicate-runs.md)）。
   そのテストに `sendDevice`、`sendAudio`、`sendAudition` のいずれかを注入する。
   名前のテストに心当たりがなければ、併記された直前のテストが待たずに残した要求を調べる。
   単体候補の追加は `opensWindow` からデバイス要求も送るため、再生なし・`preparesOutput = false` でも実 HAL へ到達し得る。
   `PluginBatchTests.invalidTargetsDoNotPartiallyInsertOrClearRedo` は `FakeParameterPlugin` を注入し、窓要求を待って対象も照合する。
   実 HAL の `write_planes` と別スレッドの過去の書込みが競合した報告は、`EchodeAppTests` からは出なくなった。
   出たら `EchodeBridgeTests` か `spike-*` など、実 HAL を使う経路を調べる。警告自体は抑制しない。
2. 終了済みと新規のスレッドの `errno` 誤検出は、C のみでも再現し、検査ランタイムの寿命処理を修正した（[ADR 0121](../decisions/0121-startup-recovery-and-errno-validation.md)）。
   `target/bridge/thread/errno/` の baseline・fixed・real-race の対照と、同じランタイムのロード証拠を確認する。
   準備が古い時は `make setup-bridge` を使う。取得済みソースの再ビルドには `python3 scripts/tsan_runtime.py --prepare` も使える。
3. 継続が `await` より先に再開されると、再開側の書込みと `await` の直後の読取りを、TSan は順序なしと見る（[ADR 0207](../decisions/0207-continuation-handoff-tsan.md)）。
   順序はハードウェアで保証され、製品の競合ではない。Swift ランタイムの即時再開の経路が TSan の acquire を呼ばないことが原因である。
   2026-10-25 に報告された `Snapshot.audioState()`（読取り）と `Snapshot.init(taking:)`（書込み）の組がこれにあたる。
   未修正では、全コアに CPU 負荷をかけた TSan の Swift 全体で毎回再現した。負荷なしではまれにしか出ない。
   文書の完了は `awaitCompletion` と `ContinuationHandoff` で、呼出しごとの前後関係を TSan へ示す。注釈が入るのは `-D ECHODE_TSAN` のビルドだけである。
   再発したら `DocumentCompletionOrderTests` と `target/bridge/thread/control/continuation-*.log` を確認する。
   回帰テストだけが落ちる時は、`-D ECHODE_TSAN` の抜けか、`awaitCompletion` を通らない経路を疑う。
   `continuation-unannotated` だけが落ちる時は、Swift ランタイムが穴を塞いだ可能性がある。注釈を外せるか調べる。
   `await` の直後に読まず、actor へ戻ってから読む経路は、切替えで順序が見えるので報告されない。報告の有無で読取りの位置を変えない。
   負荷下の全体実行で出る 5 秒のポーリングの期限切れ（`DevicePreparationTests`、`MixerStepTests`、`MixerValueBatchTests`）は別の問題である（`Status did not arrive` の項）。TSan の報告とは区別する。
   失敗した実行のログは `target/bridge/<sanitizer>/product/swift-tests.failed.log` に残り、再実行で消えない。
   抜粋には dyld の行ではなく `WARNING`〜`SUMMARY` の報告を出す。
いずれも抑制（suppression）や不要なロックで隠さず、スタックと対象アドレスを確認して製品データの競合と区別する。

### fuzz の発見（`crash-`・`oom-`・`timeout-`）

`check-fuzz` と `make fuzz` は、発見の入力を `target/fuzz/artifacts/<対象>/` に残して止まる（[ADR 0290](../decisions/0290-fuzz-targets-for-external-input.md)）。
CI の `fuzz` ジョブは最後に `xxd` で表示する。`xxd -r` でバイト列に戻せる。

1. 種類を見る。`crash-` は panic か ASan の報告、`oom-` は 2 GB を超える確保、`timeout-` は 1 入力 10 秒の超過である。
   `oom-` は、外部の値（ヘッダーの長さ・件数）で確保の大きさを決めた跡であることが多い（ADR 0290 の音声の例）。
2. 修正の前のバイナリで最小化する。修正後は再現しないので、先に行う。
   `target/aarch64-apple-darwin/release/<対象> -minimize_crash=1 -max_total_time=60 -exact_artifact_path=<出力> <入力>` を使う。
   `ECHODE_FUZZ_SCRATCH` を `target/fuzz/scratch` にすると、作業用のパッケージが一か所に集まる。
3. どこが止めたかを分ける。
   - `crates/echode-fuzz/src/` の主張（往復・保存と再読込・音声の形）: 製品が受け入れた入力で性質が崩れた。製品か対象の不備かを読む。
   - 対象の不備（例: ファイルシステムが拒む名前）は対象を直し、入力は同じく保存する。
   - 製品のコードの panic・ASan の報告: 堅牢性の不具合として直し、所属の crate に製品の境界を通す回帰を足す。
   - スタックが AudioToolbox などの OS の中だけ: OS の不具合である。製品側で避けられるかを調べ、避けられなければ ADR に記録する。
4. 最小化した入力を `crates/echode-fuzz/regressions/<対象>/` に分かる名前で置く。`tests/replay.rs` が `check` で毎回通す。
   ASan だけが見つける報告は通常テストでは再現しないので、`make fuzz` の再実行で確かめる。
