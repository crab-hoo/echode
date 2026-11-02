# デバッグ手順

目的は、製品の不具合、古いアプリ、検査の不足、取得ツールや実行環境の問題を区別し、次のエージェントが同じ入力で再現できるようにすること。

## 最初に確認すること

```sh
make agent-context
make debug-report
```

- `doctor` はソースを変更せず、アプリ・ウィンドウ・音声デバイス・プラグインを開かない。
  現在のソース、必要なツール、検査結果の有効期限（`current` / `partial` / `stale`）、ロックの保持、開発用アプリ（`target/Echode.app`）の出所を表示する（機械向けは `python3 -B scripts/diagnose.py --json`）。
- `agent-context` は同じ診断経路と検査選択を一度で使い、要約と根拠の JSON を作る。
  取得の失敗、本文予算の超過、曖昧な節、古い行番号の識別と復旧は [エージェントの手順](agent-workflow.md) を参照する。
- `debug-report` は `target/debug-reports/<日時>-<一意名>/` へ診断 JSON、案内、既存ログ（末尾最大 2 MiB）とスパイクの JSON をコピーする。
  アプリが `~/Library/Logs/Echode/` に書いた試聴の記録も `app-logs/` へ写す（素材のパスを含む）。
  生の環境変数・Cargo 設定・ユーザープロジェクトは収集しない。
- `make app` は署名後の本体・ヘルパー・同梱プラグインのハッシュとソースを `target/Echode.build.json` に記録する。
- ソースが同じでも実機計測だけが古くなったら、既定出力とレートの変更、`system_profiler -json SPAudioDataType -detailLevel mini` の読取可否を確認する。機器設定を診断のために勝手に変えない。

## 症状から調べる順番

### ビルド・環境・検査基盤

- **`make verify` の所要時間の増加**:
  `make verification-profile` で今回の実行・再利用と履歴、契約数・反例数を確認する。
  旧レポートの最後の合格時間を、今回再利用した検査の実行時間へ足さない。
  製品契約の `execution` は通常実行と反例、同期、ビルド作業秒と直列テスト秒を分ける。
  契約の秒数は自身の処理時間であり、並列の作業秒の和は全体の実測ではない。
  `normal_pending` と `negative_pending`、`reused_tests` と `reused_controls` で、通常・反例のどちらが失効したかを分ける。
  依存先の統合テストだけの変更で Swift が失効する場合は `package_scope` と明示した `inputs`、build script・custom target を調べる。
  Rust の製品ソース変更による Swift の失効はリンク先変更であり、反例の置換ファイルが不変という理由だけで全体の段階から除外しない。
  `make verify` で多数の反例が走る時は、`execution` の `changed_sites` と置換箇所を確認する。
  定義・置換元の前後 20 行・テスト本体が変わった契約だけが対象である（[ADR 0212](decisions/0212-verification-tiers.md)）。
  履歴は `target/verification/timing-history.jsonl` にあり、`make debug-report` にも収集する。
  同じ条件での再計測と、件数・失効集合の変化は [時間の保守](verification.md#実行時間の保守) に従って分ける。
  Swift 契約の直接起動で `missing helper, test bundle, or Testing framework` が出た場合は、
  選択した Xcode の `swiftpm-testing-helper` と SDK の Testing.framework、`--show-bin-path` の scratch を照合する。
  ロード失敗・終了コード 69 は期待値での反例検出ではない。古いバンドルへ戻したり 0 件を合格にしない。
  ヘルパーの実パス・バイト変更もツール識別に入り、現在の版で再検査する（[ADR 0198](decisions/0198-verification-launch-and-cache-cost.md)）。
- **`check-verification` の `VERIFICATION TIMEOUT`**:
  ログが進捗の点だけで `FAIL:` / `ERROR:` と `Ran N tests` がなければ、テストの失敗ではなく 180 秒の期限である。
  製品の不具合や変更の影響と扱わず、期限も緩めない。
  `top` で他の作業の負荷を確認し、静かになってから再実行する。すぐに繰り返しても同じ負荷なら通らない。
  2026-10-31 の短縮後は、負荷なしで約 60 秒、ビジーループ 7 本で約 108 秒である（[ADR 0208](decisions/0208-verification-shared-scopes.md)）。
  短縮前は同じ 7 本で 184〜186 秒かかり、OpenUtau が約 3 コアを使う時は 201 秒で止まった。
  これより大幅に長ければ、`python3 -m unittest discover -s scripts/tests -v --durations 12` で遅いテストを確かめる。
  実リポジトリの識別計算のテストが上位に戻った時は、範囲の共有が外れていないかを見る。
  `contract_fingerprints` 1 回は約 0.6 秒で、`scope_entries` は契約 236 件に 11 回だけ呼ばれる。
  短縮で足りなければ、測定値を添えて期限の変更をユーザーに相談する。
- **空き容量不足（`No space left on device` / `Errno 28`）**:
  `df -h` で空き容量を確認する。検査のロック解除後に、再生成可能なコンパイラーキャッシュや一時データだけを整理し、既存の検査結果・ユーザー資料・開いているアプリは削除しない。空き確保後に `make doctor` と `make verify` を再実行する。
- **検査の後に `target/debug/echode` のプラグイン処理が `control socket closed: dyld[...]: Library not loaded: @rpath/libclang_rt.rtsan_osx_dynamic.dylib` で失敗する**:
  `make rtsan` が `target/debug/echode-plugin-host` を RTSan 版で上書きした状態である。製品の不具合ではない。
  2026-11-02 から `make rtsan` は最後に通常版を作り直す。途中で失敗した時は `cargo build -q -p echode-plugin-host` で戻すか、
  `ECHODE_PLUGIN_HOST` で別のビルドを指す（[外部監査の精査](reviews/2026-11-02-external-audit-review.md)）。
- **ツール不足・ビルド失敗**:
  `doctor` の `missing_tools` と最初のエラーを読む。通常ツールは `make setup`、動的検査は `make setup-bridge`。
  ツールがそろっていても、`setup_needed` に `setup-bridge` があれば TSan の検査専用ランタイムが未準備か、
  Xcode・CMake・Ninja の版と合わない。この時 TSan の検査はソースの変更がなくても `stale` になる。
  `swift test -q` が `Build failed` / `fatalError` だけを返す場合は、`swift build --build-tests` で診断を取得する。
  同じ `--package-path`・`--scratch-path` と `-Xswiftc -warnings-as-errors` を指定し、`target/` のログで `error:` 周辺を読む。
  Xcode 側のビルド診断が quiet 出力に出ない場合がある。大量のコンパイルコマンドは会話へ展開しない。
- **Cargo の依存メタデータが欠ける**:
  `extern location for <crate> does not exist` で `target/debug/deps/*.rmeta` が欠ける時は、まず別のビルドや整理処理との競合を確認する。
  製品の型エラーと区別し、処理がない状態で欠落した依存だけを `cargo clean -p <package>` して再生成する。
  検査結果やユーザー資料は削除しない。2026-10-29 の波形キャッシュ検査では `memchr`・`errno`・`bitflags` で観測した。
  作業時の記録は `target/focused-checks/peak-cache-green/` に残し、正式な合否は `make verify` で確認する。
- **Swift の編集型の生成鮮度が不一致になる**:
  `make check-swift-actions` の `is stale; run make swift-actions` は、製品の編集拒否ではなく登録簿と生成物の差である。
  `make swift-actions` で再生成し、`EditAction.swift` の差分と `EditActionTests` の共有 fixture を照合する。
  未対応のスキーマや名前の衝突は生成器のエラーとして拒否する。生成物を手で直して回避しない。
  `typed-edit-bridge` は文書ワーカー・履歴・再オープンを検査するが、controller の辞書の移行完了の証拠ではない
  （[ADR 0202](decisions/0202-generated-swift-edit-actions.md)）。
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
- **Rust に追加したアクションを Swift が `unknown variant` として拒否する**:
  Swift のテストは Rust の製品 ABI をリンクするため、先に `make check-shell` で ABI と生成ヘッダを更新して再リンクする。
  Swift の単体実行だけでは古いアクション登録簿が残り得る。更新後も失敗する場合は、送信した `type` と登録簿を照合する。
  `echode_model_constants` の未解決シンボルや単位の不一致も、同じ製品 ABI・生成ヘッダ・再リンクから調べる。
  `ModelLimitsTests` と `ModelUnitsTests` の独立した期待値で、橋渡しと UI 消費側のどちらが違うかを区別する。
- **macOS 27 / M4 ビルド基準の不一致**:
  Cargo 生成物は `xcrun vtool -show-build <バイナリ>` と `lipo -archs` で確認する（単体の `rustc --print deployment-target` は `.cargo/config.toml` を読まない）。
  Swift は `-target-cpu apple-m4` と `-Xcc -mcpu=apple-m4` を対で指定する（[ADR 0114](decisions/0114-m4-build-baseline.md)）。
  release の proc-macro / cdylib 読込失敗（`E0463` / `mis-aligned LINKEDIT string pool`）は `.cargo/config.toml` の `strip = "none"` を確認する（[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)）。
  `scripts/check_build_host.py` が `sysctl machdep.cpu.brand_string` を読めない隔離環境では、CPU 情報の読取が許される環境で再実行する。
- **実行環境の権限制限（共有メモリ・キーチェーン）**:
  `shm_open: Operation not permitted` や `SecKeychainCreate` の失敗はサンドボックス制限によるため、許可された環境で再実行する。
  Swift のマニフェスト読込時に `ModuleCache: Operation not permitted` と標準ライブラリ読込失敗が続く場合も、先にキャッシュの書込権限を確認する。
  文書だけの検査でもビルド設定と一時署名の検査を含むため、これらの環境エラーを製品の型エラーや設計資料の不整合と混同しない。
- **長時間検査の前の準備不足**:
  `make verify` の `事前確認 ... failed` は製品テストをまだ実行していない環境エラーである。
  `target/verification/preflight-<ID>.log`、`report.md` の復旧案内を確認する。
  `make debug-report` は登録済みの事前確認ログと失敗注入の計測 JSON も収集する。
  任意の個別コマンドの出力は私的な内容を含み得るため、自動収集せず必要なログを選んで添付する。
  `tsan-runtime` は既存の準備確認で、古い文脈・マニフェスト不正・両 dylib の変更を拒否する。復旧は `make setup-bridge`。
  `bridge-nightly` / `bridge-miri` は固定 nightly・rust-src・Miri、`cargo-offline` はローカルの依存不足を切り分ける。
  事前確認を飛ばして合格にせず、復旧後に同じ `make verify` を再実行する。
- **コマンドは成功扱いなのにログに失敗がある**:
  `echo`、`;`、`grep` / `head` のパイプによる終了コードの置換を調べる。
  [個別コマンドの記録](verification.md#作業中の個別コマンド) の `commands.json` と完全ログを使い、成功表示だけを証拠にしない。
  macOS 標準 make 3.81 は `.SHELLFLAGS` を無視する。厳密なレシピ実行は Makefile の `SHELL` のオプションで行う。
- **`全体検査で未確認` の表示**:
  失敗ではなく、現在の入力で全体の段階の合格の記録がないことを示す。節目に `make verify-full` を実行する。
  ただし未実行の間に入った不具合は、節目まで検出されない。発生時期は `timing-history.jsonl` と `report.json` の `finished_at` で絞る。
- **`negative control no longer matches`**:
  製品コードの編集で、反例の置換元が現在のソースに 1 回だけ現れなくなった。ビルドの前に全反例で確認する。
  検査を緩めず、`scripts/verification.json` の `old` / `new` を本来の不具合を戻す差分へ更新する。
- **`the unmutated source copy does not build`**:
  反例を入れる前のコピーがビルドできない。反例の検出とは無関係で、コピーの同期か本体のビルドを調べる。
- **自動検査・製品契約の失敗**:
  `make verify` 失敗時は標準エラーに出力される失敗箇所の抜粋を確認し、必要なら `target/verification/<ID>.log` または `product-contracts/` の該当ログを読む。
  最適化等で `negative_control` が期待値で落ちなくなった場合は、本来の不具合を再現する差分へ更新する（例: `loop-head-residency`）。
  Swift の反例ビルドは Rust ABI を作り直さない。Rust の差分を Swift 契約へ直接登録すると、
  未変更の ABI へリンクして反例を検出できない。モデルの反例は Rust 契約、UI の反例は Swift 契約に分ける。
  `check-product-contracts`・`check-contract-controls` 全体の `VERIFICATION TIMEOUT` と、個別契約のタイムアウトを区別する。
  前者では `product-contracts.json` の各記録の `seconds` と `reused` で進んでいた契約を確認する。
  `reused: true` は同じ識別値で前回合格した記録であり、それ以外の古いログを今回の完了証拠にしない。
  一括予算は `scripts/verification.json` の各検査の `timeout`、各契約の期限は別の値である。
  Swift のコピーの初回ビルドは `SWIFT_COPY_BUILD_TIMEOUT` の範囲で行い、契約の期限に算入しない。
  検査自身の時間切れより先にフックが終了する場合は、静的なフック設定と `verification.json` の `hook_timeout` の一致を確認する。
  外側の予算は契約検査単体ではなく、全自動検査と事前確認の直列予算を覆う必要がある（[ADR 0197](decisions/0197-verification-target-scopes.md)）。
  2026-10-29 に旧 1800 秒で全件失効時の上限到達を観測し、件数と実測に合わせて一括予算を見直した（[ADR 0195](decisions/0195-verification-cost-observability.md)）。
  反例のコピーは `target/product-contract-mutations/checkouts/<N>` に残り、`<N>.json` が置いたファイルの一覧である。
  一覧が壊れていればコピーのソースを作り直す。コピーを疑う時は `checkouts/` を消せば次回に作り直す（初回ビルドの分だけ遅い）。
  Rust の反例のビルドは `target/product-contract-mutations/cargo` に置き、この検査以外から書き込まない。
  別の書き手の時刻が混じると、反例で作ったクレートを現在のソースのビルドと誤認しうる（[ADR 0148](decisions/0148-verification-pipeline-speed.md)）。
  別の実行が `target/product-contract-mutations/.lock` を持つ時は 30 秒で失敗する。`make verify` を並行して動かさない。
  `background-graph` の初期化待ちが 5 秒未満で `WouldBlock` を返す時は、accept 後の `O_NONBLOCK` を調べる。
  Darwin は非ブロッキングのリスナーのフラグを受け継ぐため、読込み期限の設定だけでは待機に戻らない。
  検査用の `initializer_peer` と、送信前のフラグを確認する通常回帰を使う（[ADR 0142](decisions/0142-background-plugin-graph-preparation.md)）。
  2026-10-30 の編集型生成の検査では、`mcp-backend-framing` の正常回帰が 5 秒の EOF 待ちで一度失敗した。
  `handshake=true`・改行なし・`following=""`、Darwin errno 35 であり、反例の検出とは区別する。
  当初は同じ Cargo 構成の単独実行では再現せず、原因は未確定だった。この時点では MCP の製品コード・期限・回帰は変更していない。
  初回の失敗と単独再実行は `target/focused-checks/typed-edits-mcp-framing/` に残す。
  再発時は接続の所有権・半閉鎖後の読取り・実行中のプロセスの状態を採取し、正式な `make verify` と区別する。
  後続のピアノロール移行で再発し、fixture の `Value::Display` の直接書込みでは反復して再現した。
  製品と同じ先行文字列化と `write_all` へ変えると反復では再現しなかった。OS 内部の原因は未確定である。
  送信内容・半切断・期限・アサーションは維持し、サーバーとクライアントの時間・出力を失敗時に残す
  （[ADR 0115 の fixture 補強](decisions/0115-phase2-mcp.md#半切断-fixture-の送信と診断の補強)）。
  同じ作業の次の実行では、通常回帰後・Rust 反例 95 件の完了後、最初の Swift 反例ビルド群が個別期限に達した。
  全体は約 954 秒で、3600 秒の一括予算到達ではない。群の強制終了前のコンパイラー出力は保存されていない。
  `ArrangeAudioDrop.swift` と `ArrangeClipMove.swift` の 3 コピーが対象で、残留プロセスはなかった。
  中断時点は同じ作業ログの `counterexample-timeout.log` に記録し、対象や期限は変えず未完了分を再実行する。
  2026-10-31 に新しい worktree で同じ位置（Rust 反例 101 件の後）の停止を再現した。コピーに Swift のビルドがなく、最初の群が 3 コピー同時の初回ビルドになり、180 秒の個別期限に達する。
  完了した反例の記録と途中までのビルドは残るので、再実行で先へ進む。追加のビルドは 1 コピー約 25 秒だった。製品やテストの失敗と区別する。
  新しい worktree ではこの検査だけで数 GB を超え、Rust 反例の `cargo` は一時 26 GB に達した。空きが少ない時は、記録済みの反例の `cargo` を消して再実行できる。
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
  製品のテストを通すために workspace 全体でこの feature を有効にしない（[ADR 0211](decisions/0211-opt-in-history-spike.md)）。
  エンジンの `echode-engine-spike` が同じ理由で拒否された場合も、`--features spike` の指定漏れである。
  通常回帰は `cargo test -q -p echode-engine --features spike --lib`、実機計測は明示的な依頼がある時だけ `make spike-engine-smoke`・`make spike-engine` を使う。
  製品の `playback`・機器一覧・Workgroup の問い合わせのビルドに、この feature は不要である
  （[ADR 0213](decisions/0213-opt-in-engine-spike.md)）。
  単独では通る保存・復旧の回帰が、復旧後のトラック数などの過剰で落ちる時は、`$TMPDIR` に残った同じ pid の一時パッケージを疑う。
  失敗した過去の実行が残したジャーナルが再生され、2026-10-21 に `phase1-acceptance` で再現した。
  一時パッケージを作る `scratch()` は使用前に同じパスを削除する。
- **ASan / TSan の Swift 検査で `Status did not arrive`（復旧の警告待ちの期限切れを含む）**:
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
  3.1 秒を超える回は 2026-10-31 の main（05dda77）の上でだけ見えた。差の原因は調べていない。
  長い停止は実行直後の AppKit の遅延読込みと初回のビュー構築で、入力欄のフォーカスや `NSMenuItem` の実行を契機に数百のフレームワークが主スレッドで読み込まれる。
  長い停止の標本には複数のテストが並ぶので、1 つのテストの占有とは限らない。TSan の負荷なしで 6.7 秒の停止があった回も合格したので、停止の長さだけで判断せず標本の主を見る。
  `DYLD_PRINT_LIBRARIES` の出力は原因ではない。外しても同じように失敗し、占有の長さも変わらなかった。
  負荷の下で残った期限切れの主因は全テストの同時実行で、2026-11-01 に合否を決める実行の同時実行数を 16 に絞った（[ADR 0210](decisions/0210-sanitizer-test-width.md)）。
  絞らないと約 260 個のテストが同時に生き、`await` やポーリングの 1 回ごとに全テストの一巡を待つ。
  飽和の下の主スレッドの時間は、ファイル削除が約 3 割、フレームワークの読込みが 3 割弱、開閉ボタンの入れ子のランループが約 5% を占めた。
  同じ負荷（スピナー 10 本、各 30 回）で、`the unheld device parameter reply` の待ちの中央値は 2.64 秒から 0.01 秒になった。
  任意の 5 秒の中の停止の合計は、全回 4.5 秒以上から全回 2.67 秒以下になった。
  失敗の有無は、この負荷では前後とも 30 回中 0 回で差が出ない。
  3 倍の負荷（スピナー 30 本、各 10 回）では 10 回中 6 回から 10 回中 0 回になった。
  ただし、その負荷の後でも任意の 5 秒の中の停止の合計は 3.9〜4.85 秒に達する。
  全テスト同時の実行は TSan の競合を探すために残し、テストの失敗を判定しない。
  `race-swift-tests.failed.log` のテストの失敗は、サニタイザの報告がなければこの検査の失敗ではない。
  `check_test_width: N tests ran at once, above 16` は、環境変数が効かなくなったことを示す。比較の前提が崩れるので、ADR 0210 に戻って手段を決める。
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
- **中断後に `running` が残る・全結果が一度に `stale` になる**:
  呼出し元の時間切れは検査自身の終了コードではない。ロックが空いていても、自分が起動した make / cargo / テストの子が残る場合がある。
  `ps` と保存ログで所有関係と終了を確認し、残った検査と次の検査を重ねない。既存アプリや他の作業のプロセスは終了しない。
  子のログが後から成功しても、親が完了を記録していなければ合格へ手で移し替えず、`make verify` を再実行する。
  検査の OS 識別はホスト名を除外するため、ネットワークによる名前変更だけでは失効しない。
  OS・カーネル・アーキテクチャやツールの変更は引き続き失効させる（[ADR 0195](decisions/0195-verification-cost-observability.md)）。
  2026-10-28 の ADV-04 検査では、旧実装のホスト名による全件失効と中断の組合せを観測した。
  当時のログと、現在の入力に結び付いた有効な検査記録を区別する。
  Swift の負の対照を同じ位置まで何度もやり直す場合は、契約ごとの完了記録が増えているかも確認する。
  各契約の全反例が終わった時点で保存する。一部の反例だけが終わった契約は合格ではない（[ADR 0148](decisions/0148-verification-pipeline-speed.md)）。
- **負の対照の `NOT rejected` / `Build failed`**:
  反例は、ビルドが通り、テストの `Expectation failed` で落ちた時だけ拒否と数える。
  コンパイルエラーと `Fatal error` のクラッシュは、アサーションが欠陥を捕まえた証拠にならない。
  逃げるクロージャ内の `self.` の欠落や、境界値での `Int64(Double(...))` のトラップを `<契約>-negative.log` で確認し、反例側を直す。
- **基準コミット（`VERIFY_BASE`）エラー**:
  一時リポジトリのテストでは親の `VERIFY_BASE` を隔離する。無効な ref 指定は HEAD へ黙って置換せずエラーにする。
- **終了フック直後の不要な再実行**:
  ツール識別は PATH 文字列ではなく解決された `rustc` / `swift` 等の実パスを使う。

### オーディオ・トランスポート・プラグイン

- **監視の即時破棄回帰が `drop did not return within 3 s` で落ちる**:
  `audit_plg9_a_watch_can_be_dropped_right_after_it_starts` の期限は、監視の開始と破棄の両方を含む。
  2026-11-02 の SPG-09 検査では全体と単独で失敗した後、同じ期限の単独実行が約 97 秒で完走した。
  成功した診断実行の標本は `FolderWatch::start` → `FSEventStreamStart` → `register_with_server` → `mach_msg` であり、破棄の待ちとは限らない。
  初回の失敗位置と OS 側の根因は未確定。既修正の run-loop 開始競合と決めつけず、開始・停止のどちらかを標本で区別する。
  期限・対象は変更せず、採取ありの診断と正式な検査を区別する。証拠は `target/focused-checks/spg09-watch-timeout.md` と `spg09-watch-recheck/`。
  SPG-06 の検査でも単独実行は約 100 秒で完走した一方、全 audit と `make verify` では同じ期限で失敗した。
  失敗した全 audit の実行中の標本も開始 RPC の待ちであり、停止処理のハングと断定しない。
  記録は `target/focused-checks/spg06-watch-diagnostic.log` と `spg06-watch-sample-52165.txt`。
  標本は期限切れの瞬間を特定していないため、OS 側の根因は引き続き未確定である。

- **ヘルパーの保存・復元・パラメーター・窓の要求が時間切れ／busy になる**:
  `plugin main request N timed out; outcome unknown` は実行の取消ではない。
  メインスレッドに残る要求が完了するまで後続は busy で拒否し、自動再送しない。
  要求番号 N はヘルパー内部の呼出し番号で、ソケットの応答照合の番号とは別である。
  `plugin-main-timeout-state` は機器なしの `slow-save` で再現し、`plugin-control-request-ids` は古い応答の破棄を検査する。
  完了後の復元バイトと新しい保存結果を比較し、実行中の拒否と、古い応答の取り違えを区別する。
  操作と通信の対応・Unicode と状態バイト・拒否理由は `session_operations_tests.rs` で切り分ける。
  実装は `session_operations.rs` の `PluginControl` にあり、ホスト経由の保存・復元の別入口はない
  （[ADR 0227](decisions/0227-control-only-plugin-operations.md)）。
  親とヘルパーは同じビルドでそろえ、古い起動済みアプリを新しいヘルパーだけに置き換えない
  （[ADR 0199](decisions/0199-owned-helper-control-requests.md)）。

- **ノートの端から再生すると、その音が鳴り続ける**:
  開始位置がノートの終端ちょうどかを確認する。追いかけ（`Cursor::chase_events`）は終端ちょうどのノートを鳴らさず、
  その時刻のノートオフだけを出す（[ADR 0164](decisions/0164-note-end-chase.md)）。
  同じ時刻ではノートオフがノートオンより先に並ぶため、追いかけのノートオンがあると鳴り続ける。
  必須契約 `note-end-start` は実ヘルパーで位置移動と停止の復帰を確認する。計画の交換中の場合は `release_replaced` を別に調べる。

- **壊れた素材を読むと `attempt to multiply with overflow` / `capacity overflow` で文書が使えなくなる**:
  `media.rs::decode_pcm` のヘッダ長の計算を確認する。
  BLAKE3 が一致しても、PCM の形式まで有効とは限らない。
  `audit_cur01_malformed_pcm_length_returns_an_error_without_panicking` が、24 バイトの不正ヘッダで再現する。
  現行の回帰は debug / release の両方で、サンプル数・バイト数・ヘッダ加算の桁あふれをエラーとして拒否する。
  当時の panic の位置は [CUR-01](reviews/2026-10-25-code-audit.md#cur-01-pcm-のバイト長計算が桁あふれする) を参照する。

- **保存済み素材だけが `audio contains a non-finite sample` になる**:
  ヘッダ長ではなくサンプル値の検証による拒否である。正しいハッシュも値の妥当性を保証しない。
  取込み元の復号と保存済み PCM の全読込みを区別し、後者は `canonical-pcm-validation` で再現する。
  トリム外の不正値も拒否するが、有限の振幅 1 を超える値は不正としない。原本を自動で書き換えない。

  外部 WAV の取込み・一括復号の拒否は `audio-import-validation` で別に検査する。
  逐次復号の `AudioFileReader::read` が拒否しても、一括の `decode_file` に同じ検証がある証拠にはならない。
  取込み時の拒否はパッケージへの書込み前、保存済み PCM の拒否は読込み時であり、同じエラーでも段階を区別する。

- **波形が消える・キャッシュ破損後に表示が変わる**:
  `cache/peaks/<hash>` は派生データであり、段構成・バイト長・有限で順序の正しい列を検査する。
  不正なら `Waveforms::load` が正準 PCM から作り直す。再生成できなければ波形なしにするが、原本は変更しない。
  `peak-cache-validation` は公開読込みと再生成、`peak_cache_validation.rs` の通常回帰は PCM なしの正常なキャッシュも確認する。
  正常なキャッシュで粗い列だけを描けても、PCM の全体が読める証拠にはならない（[ADR 0058](decisions/0058-waveform-peaks.md)）。
  有限の大きなステレオ素材だけが無音の波形になる時は、平均の加算が `f32` へ戻っていないかを確認する。
  構築とマップの両方が対象で、`stereo-waveform-average` が極大値・非正規化数と旧キャッシュの再生成を区別する。
  `ECHDPK01` は再利用せず PCM から `ECHDPK02` を作るため、素材がない旧キャッシュは波形を出さない。
  素材の自動修復や再生音の変更ではない（[ADR 0058 の後続修正](decisions/0058-waveform-peaks.md)）。
  境界が 1 列の途中にある時だけ包絡が違う場合は、`source_span` の素材範囲と `includes_silence` を調べる。
  素材前・EOF 後・配置窓の外側を切り詰めても、その無音は列の最小・最大へ含める必要がある。
  `waveform-boundary-silence` は正負の包絡・粗い列とマップの端・テンポ変化・キャッシュ再利用を区別する。

- **S2 / S3 は通るのに製品だけ負荷に弱い**:
  選択デバイスの Workgroup 取得・ヘルパー参加・変更通知後の再準備を確認する（[ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md)）。`plugin_missed_blocks` と実機条件を記録する。
- **再生開始が遅い・準備中に停止できない**:
  `pending-transport`（UI 受付）、`queued-stop`（ABI 投入時取消）、`transport-cancellation`（HAL 初回発音禁止）の 3 境界を切り分ける（[ADR 0097](decisions/0097-cancellable-retained-transport.md)）。
- **再生中のぶつぶつ・停止時のクリック**:
  `audio.status` の `plugin_missed_blocks` と `pending_plan` を確認する。
  定期保存確認による一時バイパス排除（[ADR 0101](decisions/0101-live-output-discontinuities.md)）、音声側の排他グラフ所有（[ADR 0102](decisions/0102-exclusive-audio-graph.md)）、5 ms の停止波形減衰（[ADR 0103](decisions/0103-live-stop-waveform.md)）、待機しない後片付け（[ADR 0105](decisions/0105-nonblocking-stop-cleanup.md)）のどの境界かを確認する。
  [ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md) 以降は素材をプラグインの手前で減衰し、マスターの停止フェードは使わない。
  `stop_fade_offset` は素材の減衰位置である。停止後も余韻・自発的な発音が出ることは正常で、停止中の `plugin_missed_blocks` も実際の処理失敗を表す。
- **停止中にプラグインの時間・鍵盤の発音が止まる、または余韻が切れる**:
  `playing = false` とライブ処理の停止を混同しない。保持した出力は無音入力の処理を続ける。
  `stopped-plugin-output` で実ヘルパーと後段 FX の出力、`stopped-plugin-tails` でノートオフと自然な余韻の終端を確認する。
  音源・FX の `bypassed` / `offline`、出力が既に開いているか、位置移動・準備取消でリセットが要求されていないかを分ける。
  停止だけで `cleanup_pending` が立つなら旧仕様か不具合を疑う。位置移動の後片付け中なら正常である。
  位置移動直後の停止で期限切れになる場合は `stop-cleanup` を使い、保留中に通常の素材処理を始めていないか確認する。
- **停止で開始位置へ戻らない・戻る時に余韻が切れる・開き直すと開始位置が消える**:
  `audio.status` の `tick`（現在位置）、`start`（記憶した開始位置）、`paused` を分けて読む（[ADR 0155](decisions/0155-remembered-playback-start.md)）。
  停止は `Rewind` で時計だけを戻す。余韻が切れるなら停止の経路が `Locate`（リセットあり）になっていないかを `stop-return-tails` と `playback-start` で切り分ける。
  開始位置はパッケージ直下の `transport.json` にあり、履歴・Undo・復帰の対象外である。
  壊れた・新しい版・リンクのファイルは 0 として読むので、開始位置が 0 に戻る時はこのファイルの中身と種類を確認する。
- **ループ範囲のドラッグで再生位置が動く・終端以降から勝手に戻る**:
  押下では開始位置を送らず、離した時のクリックだけが `transport.set_start` を送る。
  ドラッグの範囲はオフ中は下書きだけ、オン中は `transport.loop` を 1 回送る（[ADR 0156](decisions/0156-ruler-loop-range-and-entry.md)）。
  `piano-ruler-loop-ui`・`arrange-ruler-ui`・`arrange-ruler-cancel-ui` で入力と取消、`loop-entry` で音声側を切り分ける。
  範囲欄のフォーカス移動・オンオフで端点が変わる場合は、`ProjectController.loopRange` と `LoopEntry.range` を比較する。
  下書きと送信は整数で共有し、未変更の表示文字列を拍から再変換しない。
  `integer-loop-draft-ui` は Double の精度外の端点も製品要求まで保持することを検査する。
  手入力した欄の不正値・ゼロ長は拒否し、反対端を丸めたり左右を交換したりしない
  （[ADR 0201](decisions/0201-integer-loop-drafts.md)）。
  既存端の調整は上端 8 pt のグリップから行う。画面外の端を、帯の切れ目でつかむことはできない。
  片端だけの調整にならない場合は `RulerHandles.hit` と `RulerPress.edge`、つかんだ範囲とポインターのずれを確認する。
  別エディタ・範囲欄・オンオフの変更で下書きが消えるのは、古い解放による上書きを防ぐ正常な取消である。
  `loop-handles-ui` と `loop-handles-cancel-ui`、各オフスクリーンプローブで入力・取消・描画を分ける。
  アレンジの行だけがずれる場合は `arrangeTop` を描画とヒットから二重に足したり除いたりしていないかを確認する。
  `spike-arrange-frame` の `rulerMismatches` は帯と印の画素不一致、`maxHeaderLaneError` は上端補正後の左右の誤差である。
  `audio.status` のループがオンでも、終端以降へ置いた再生は通常進行する。
  ブロック処理で終端に着いた場合は次のブロックで戻るため、同じ値の `tick` でも位置移動と混同しない。
- **ピアノの入力先・参照表示とアレンジ選択が一致しない**:
  通常のアレンジ選択は入力先を変えない。右の「編集」かアレンジの空白・MIDI クリップのダブルクリックで明示的に開く。
  `pianoTrack` と `selection`、操作時の `PianoTrackTarget.document`、公開スナップショットの現存トラックを分けて調べる。
  参照は `PianoRenderInput.references` と同じフレームの一覧を照合し、ヒット判定・ベロシティ・読み上げに混ざっていないか確認する。
  `piano-target-ui`・`piano-arrange-open-ui`・`piano-reference-ui`・`piano-track-selector-ui` で切り分ける。
  ダブルクリック後の再生成で位置が戻る時は `pianoLocationID` の消費を見る。通常の切替では共有カメラを保つ。
  `spike-piano-frame` の `checkedReferenceNotes` と `mismatches` は倍率 1・2 の参照色・編集優先とレーンへの非混入を含む。
- **矢印の連続入力で一部だけ動く・前の状態へ戻る**:
  `enqueuePianoStep` のイベント順と `PianoStep` の相対差分、待機列の手番での全 ID 検証を確認する。
  ドラッグとは異なり、押下時の絶対幾何を再使用しない。共通上下限とリンク中身の重複排除は同じである。
  `piano-key-step-ui`・`piano-key-step-bounds-ui` で保存・履歴・境界と旧文書の拒否を確認する。
- **Return が挿入する・一時停止しない・一覧や入力欄から再生へ漏れる**:
  ピアノは Return とテンキー Enter をレスポンダーへ渡し、メニューの共通 Pause に送る。
  サンプル・FX 一覧のフォーカス時は `performKeyEquivalent` が配置を先に受け、入力欄ではメニューを無効にする。
  `piano-return-ui`・`browser-return-ui` と `plugin-key-events`・`plugin-key-socket`・`plugin-key-ui` で経路を分ける。
  実画面のメニュー・IME・VoiceOver・キーリピートと聴感はユーザー確認に残す（[ADR 0181](decisions/0181-piano-reference-workflow-and-keys.md)）。
- **ピアノロールの枠選択・削除が想定と違う**:
  右ドラッグはノートグリッド内から始める。下部の棒・鍵盤・ルーラーからは始めず、枠は発音窓内の可視ノートとの部分交差を使う。
  `PianoGridView.selectionPress` が仮の枠（`PianoInteraction` の射影）、`ProjectController.noteSelection` が確定選択である。
  複数選択の時の `noteFocus == nil` は正常で、単一ノート欄や記憶値へ任意の代表を流さないための区別である。
  Delete は `deleteNotes` の待機列の手番で全 ID を検証する。リンク配置の二重削除と、古いメニュー・旧文書の拒否を分けて調べる。
  `piano-note-selection-ui`・`piano-note-selection-cancel-ui`・`piano-note-selection-delete-ui`・`piano-note-linked-delete-ui` を使う。
  選択済み本体は一括移動、端は一括伸縮する。未選択ノートの通常押下は単一選択へ戻る。
  実画面の枠とメニュー、VoiceOver の操作感はユーザー確認とする（[ADR 0176](decisions/0176-piano-note-selection-and-deletion.md)）。
- **Command ドラッグが元を動かす・選択が消える・強弱が変わる**:
  押下時の `PianoMoveGesture.copies` と解放時の `PianoMoveChange` を確認する。Command は押下で保持し、Shift と位置は解放時を使う。
  クリックだけなら解放時に選択切替、ドラッグなら端からでも複製し、成功後の `selectingNotes` で複製側を選ぶ。
  `note.duplicate` は共有内容の元を複製するため、リンクの同一ノート ID は一度だけ送る。強弱を 7 bit の `note.insert` で作り直さない。
  `note-duplicate-model`・`piano-note-duplicate-ui`・`piano-note-duplicate-targets-ui`・`piano-note-duplicate-cancel-ui` で切り分ける。
  固定窓を広げた時の重なりは従来どおり全体を拒否する。実画面・VoiceOver・聴感はユーザー確認とする
  （[ADR 0184](decisions/0184-piano-note-duplicate-drag.md)）。
- **複数選択の移動で間隔が崩れる・一部だけ動く・確定されない**:
  選択済みノートの本体を通常ドラッグする。3 pt 未満の微動とクリックでは編集せず、端と Command は別操作である。
  `PianoMoveGesture.originals` が画面外も含む押下時の全対象、`PianoMovePreview` がモデルを変えない全対象の仮表示である。
  つかんだ開始の絶対吸着から差分を求め、全対象の開始・終端・最低音・最高音で共通制限する。上下端で全体が止まるのは正常である。
  解放時の座標・Shift と、`PianoMoveChange.actions` の手番での全幾何検証を分けて確認する。
  `piano-note-move-batch-ui`・`piano-note-move-batch-targets-ui`・`piano-note-move-batch-cancel-ui` を使う。
  クリップ同士の重なりはまだモデルが全体を拒否する。他の固定リンクのトリムは保つため、共有内容の変更と窓の拡張を混同しない。
  重なり拒否の再現では配置を固定して分ける。隣接小節へ順に入力するだけでは自動配置が結合し、壁を持つ別配置にはならない。
  入力長・入力ベロシティは変えない。実画面と VoiceOver はユーザー担当である（[ADR 0178](decisions/0178-piano-batch-note-movement.md)）。
- **複数選択の伸縮で長さの差が変わる・一部だけ伸びる**:
  選択済みノートの左右端を通常ドラッグする。3 pt の横移動までは確定せず、縦移動は音高を変えない。
  `PianoResizeGesture` はつかんだ端の絶対吸着から共通差分を求め、画面外も含む最短長・最小開始・最大終端で制限する。
  最短 1 tick は Shift の有無にかかわらず共通制限する。長さの比率を保つ処理ではない。
  `PianoResizeChange.actions` の全幾何検証・リンクの重複排除と、固定窓の拡張・重なり拒否を分けて調べる。
  `piano-note-resize-batch-ui`・`piano-note-resize-batch-targets-ui`・`piano-note-resize-batch-cancel-ui` を使う。
  入力長と入力ベロシティを保ち、解放時に 1 回だけ記録する（[ADR 0179](decisions/0179-piano-batch-note-resizing.md)）。
- **複数選択のベロシティが動かない・強弱差が変わる**:
  選択済みのノートを Option ドラッグ、または選択済みの棒をドラッグする。未選択の押下は単一選択になる。
  差分は選択全体の最小・最大値で制限し、どれかが 0／127 に達したら全体が止まる。両端の値を含む時は動かないのが正常である。
  `PianoVelocityGesture.originals` は押下時の全対象、`PianoVelocityPreview.values` は共有ノート ID ごとの仮の値である。
  `PianoVelocityChange.actions` は待機列の手番で存在と元の保存値を検証し、1 つでも古ければ全体を拒否する。
  取消と古い対象の拒否、共通上下限、入力値だけの応答の世代を `piano-velocity-batch-ui`・`piano-velocity-batch-targets-ui`・`piano-velocity-batch-cancel-ui` で区別する。
  入力長は変えず、入力ベロシティだけがつかんだ音の変更後の値になる（[ADR 0177](decisions/0177-piano-batch-note-velocity.md)）。
- **トラック切替でピアノロールの表示範囲が変わる**:
  文書内の共通カメラは `CanvasNavigationState.pianoCamera` で、トラック別の復元は行わない。
  切替先の既存選択への自動表示と、切替後の Tab・編集・再生ヘッド追従による移動を区別する。
  `piano-shared-viewport-ui` は切替・再生成・空トラック集合・旧文書の書戻し拒否と別文書の初期化を確認する。
  別文書で位置が残る場合は `setNavigation` の文書 ID と `prepare` の初期化、書戻しの `permits` を見る
  （[ADR 0174](decisions/0174-shared-piano-viewport.md)）。

- **再生ヘッドに表示が追従しない・勝手に戻る**:
  まずトランスポートの「再生ヘッドに追従」がオンかを見る。横方向が優勢なスクロールは正常にオフにする（[ADR 0159](decisions/0159-playhead-follow.md)）。
  オンでも動かない時は、ルーラー・ノート・クリップの押下や数値入力欄、音声ファイルのドラッグによる保留中かを確認する。
  停止中のズームで範囲外へ出た再生ヘッドは、次の再生・停止・位置移動まで表示しないのが仕様である。
  再生中に止まる場合は `PlayheadFollower.awake` と、`PlaybackFeed` の `playing`・`tick` が進んでいるかを分けて見る。
  `playhead-follow-ui` は判定・保留・解除、`playhead-follow-state-ui` は文書ごとの値を切り分ける。
- **音源なしのノート・Sine Pad が鳴らない**:
  音源のないトラックは無音である。デバイス一覧に `dev.echode.sine-pad` があるかを確認する。
  一覧にない時は `make bundled-plugins` とヘルパー隣の `plugins/`、アプリ内の `Contents/PlugIns/`、同梱用走査キャッシュを確認する。
  `host event capacity was not prepared` はイベント領域の準備漏れであり、音声側の確保や固定発音数で回避しない
  （[ADR 0123](decisions/0123-explicit-sine-pad-plugin.md)）。
- **同梱 CLAP の初回だけ RTSan が malloc を検出する**:
  子プロセスの標準エラーを収集し、`_tlv_get_addr` と `ForbidAlloc` が並ぶかを確認する。
  dylib 固有の検査用 TLS は、ヘルパーの最初の非 RT 起動段階の `start_processing` で初期化する。
  検知を外さず、未発音の新しいヘルパーで最初の処理も検査する。
- **CLAP が鳴らない・走査失敗**:
  プラグイン ID、バンドル走査（macOS の `.clap` ディレクトリを含む）、キャッシュ、ヘルパー起動、入出力バスを追う。走査失敗時は子の終了コードとシグナルを確認する。本体プロセスにプラグインをロードして切り分けてはいけない。
  2026-10-28 の ⌘D 変更の通常検査では、`audit_rea08_descendant_pipe_does_not_extend_deadline` の最初の JSON 出力が 2 秒の期限切れとなった。
  失敗は子孫のパイプ保持ケースではなく、最初の短い shell の起動・完了待ちで、単独実行では再現しなかった。原因は未確定である。
  個別のログは `target/focused-checks/clip-duplicate-scan/`。期限・対象を変更せず、正式な `make verify` の結果と区別する。
  ミュート／ソロの一括操作の検査で再発し、期限切れの子は `running`・両出力 0 byte で、採取では `_dyld_start` にとどまっていた。
  さらに、従来の子孫保持ケースが shell の起動待ちだけで合格し得たため、専用のネイティブ検査ヘルパーへ変更した。
  製品の期限は維持し、出力済み・直接の子の終了も検査する。期限切れの PID・終了状態・byte 数・パイプの開閉で、起動前と保持を区別する
  （[ADR 0190](decisions/0190-native-scan-deadline-fixtures.md)）。OS 側の起動遅延の根因は未確定である。
  2026-10-30 の検査コスト改定でも、ネイティブ fixture の最初の正常 JSON が同じ 2 秒の期限切れとなり、単独でも一度再現した。
  子は `running`・両出力 0 byte で、直後の同じバイナリの直接起動は約 8 ms、同じ Cargo 構成の再実行は約 0.45 秒で完了した。
  shell 除去でこの環境の起動遅延自体が解決したとは扱わず、期限は維持する。
  初回の失敗は検査時間履歴と作業ログへ残し、再実行の合否で過去の失敗を上書きしない。
- **バイパスの表示と音が一致しない・解除後に古い音が鳴る**:
  文書のデバイス ID と `bypassed`、計画の要求・適用版、プラグイン失敗、ドライ／ウェットの平面数を確認する。
  バイパスした音源ではノートが無音でも同じトラックの配置音声は鳴る。解除時のリセットと現在のノート追跡を確認する（[ADR 0120](decisions/0120-device-bypass.md)）。
- **オフラインの FX が読み込まれる・素材まで無音になる・書出しに失敗する**:
  文書の `offline`、`bypassed`、`state` を分け、保存形式 5 と旧版からの変換を確認する。
  `fx-offline-ui` は実ボタンと履歴、`plugin-offline` は隔離グラフと記録済み状態の復元、`offline-export` は実 CLI のログである。
  `chain_of` と `project_plugin_ids` / `routes_with_saved_state` の対象、計画の要求・適用版を確認する。
  `device rendering requires the isolated plugin host` が全オフライン時に出たら、ホストなし書出しの対象判定を確認する。
  再有効化後の未検出・状態オブジェクトの読込エラーとは区別する（[ADR 0125](decisions/0125-device-offline.md)）。
- **パラメーター一覧が出ない・値が音に反映しない・Undo で音が戻らない**:
  デバイスの列の表示文が境界を示す。`the project has no running plugin` と `the device has no running plugin` は文書・オフライン・未検出、
  `no output device is available` は停止中の出力起動、`parameter changes timed out` はヘルパーの処理スレッドの `flush` を疑う。
  `document thread stopped` は文書を閉じた後の要求か、ワーカーの異常終了の後の要求、`device request panicked` はデバイスレーンでの異常終了である。
  `document thread panicked` は文書の手番の panic で、以後の要求は `stopped` になる。stderr の `echode-worker` の panic が原因を示す（[ADR 0204](decisions/0204-document-service.md)）。
  完了が返らない要求は `completion_tests.rs`、`failed to join thread: Resource deadlock avoided` は `document-lane-completion` で切り分ける。
  `echode-ffi` のテストが `removal index ... should be < len` と `panic in a function that cannot unwind` で abort したら、完了コンテキストの寿命を疑う。
  コールバックの `send` が走っている間に待つ側が抜けられるので、コンテキストはスタックに置かず、コールバックが `Box` で所有する。
  一覧取得の失敗は列の「パラメーターを再読込み」で対象だけを再試行する。検索・選択・履歴は変えず、再失敗も同じ列に出す。
  プラグインの再起動や状態の再読込とは異なるため、クラッシュしたヘルパーや未検出のプラグインの復旧を保証しない。
  再試行で他の列も消える・連打で要求が重なる場合は `device-list-retry-ui` と `DeviceParameterRetryTests` を使う
  （[ADR 0140](decisions/0140-device-parameter-list-retry.md)）。
  `plugin-params` は実ヘルパーとエンジン、`fx-params-ui` は画面と履歴のログである。
  Undo では記録のハッシュとヘルパーが最後に保存・読込したバイトを比べ、一致すれば読み込まない（[ADR 0130](decisions/0130-clap-parameter-editing.md)）。
- **空白への移動・再開で以前の残響が鳴る**:
  停止・シーク時の `active.reset()`、`NOTE_CHOKE`、宣言された全 MIDI ポートへの CC120 / All Notes Off を確認し、自然な余韻と区別する（[ADR 0099](decisions/0099-transport-plugin-state.md)）。
  ライブ停止では余韻を残す。上の消去は位置移動・未発音の準備取消とオフライン停止の検査であり、ライブ停止へ戻さない
  （[ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
- **左右の消失・定位変化・ループやフェーダーの段差**:
  逆位相・異振幅の左右信号で素材読込・エフェクト間・フェーダー後を照合する（`stereo-media`、`stereo-plugin-chain`、`audio-loop-seam`、`audio-fader-ramp`、[ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)）。
- **ループ変更での再読込・トリム位置ずれ**:
  `clips.rs` の保持キーと `CachedTrack` の失効条件を確認する（[ADR 0108](decisions/0108-phase1-loop-readiness.md)）。内容位置 0 は `start - offset` のテンポ積分で、負のオフセットは無音となる（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
- **長時間・短時間の実機音声計測の失敗**:
  処理期限超過（`late`）と HAL 呼出欠落（`gaps`、`pmset -g log` の Sleep / Wake や `/usr/bin/log show` の coreaudiod デバイス変更）を区別する（[ADR 0096](decisions/0096-measurement-completion.md)）。短時間確認は `--native-rate` を使う。

### UI・描画・入力・履歴・MCP

- **JSON-RPC が `-32600` を返す・ID なしの入力にもエラーを返す**:
  `rpc.rs::ApiSession::reply` の外枠検証を確認する。
  `jsonrpc` と `method`、ID の型、明示した params がオブジェクト・配列かを照合する。
  明示した `params:null` は省略と異なり不正で、メソッドの引数エラー `-32602` と区別する。
  正常な通知だけ応答を省略し、壊れた外枠は ID がなくてもエラーにする。
  `request_validation.rs` の公開 API と実ソケットの回帰で、拒否時のジャーナルと Undo / Redo 保持、
  後続要求の応答を確認する（[ADR 0020](decisions/0020-json-rpc.md#要求の外枠の検証-2026-10-29)）。

- **JSON-RPC の前後に空白がある時だけ `-32700` になる**:
  JSON の空白は SP・HT・CR・LF に限られる。垂直タブ・改ページ・ノーブレークスペース等は、見た目が空白でも不正である。
  `rpc.rs::ApiSession::reply` が Unicode 全体の `trim()` で不正文字を消していないか確認する。
  文字列の外側の構文エラーと、名前など文字列内の正当な Unicode を区別する。
  `request_whitespace.rs` と必須契約 `rpc-json-whitespace` は公開 API と実ソケットで、拒否後の履歴と後続要求を検査する。
  入力を自動修正して再送せず、元の入力と応答を照合する（[ADR 0020](decisions/0020-json-rpc.md#json-の空白だけを除去する2026-10-29)）。

- **API が `request line ended without a newline` で接続を閉じる**:
  EOF の前に LF が届いていない。JSON 自体が正しくても、この部分行は編集・通知・Undo / Redo へ渡さない。
  `ApiSession::reply` の直接呼出しと、`serve_connection` の通信境界を区別する。
  完了済みの前方の要求は成立している場合があるので、接続全体を未実行と決めて再送しない。
  `request_framing.rs` と `rpc-request-framing`、`serve_tests.rs` の行上限の境界で再現する
  （[ADR 0073](decisions/0073-socket-path.md#改行前の切断の拒否-2026-10-29)）。

- **素材やキャッシュが Git の除外対象にならない**:
  パッケージ直下の `.gitignore` に `/media/objects/` と `/cache/` の正確な規則行があるかを確認する。
  コメントや似たパスを部分文字列で照合すると、取込みとホスト開始で不足規則の追加を省略してしまう。
  `store-gitignore` は両入口と既存テキスト保持を検査する。任意の否定規則による上書きの解析は対象外である
  （[ADR 0031 の後続修正](decisions/0031-content-addressed-store.md#除外規則の行単位の照合-2026-10-29)）。

- **保存して開き直すと小さいゲインやパンが変わる・0 になる**:
  編集直後の値と、`tracks/<id>.json` の数値、再読込後の Float のビットを比較する。
  旧 formatter の小数点以下 9 桁への丸めが原因なら、ジャーナルでは値を保持してもパッケージで失われる。
  `package-float-roundtrip` でゲインと負のパン、`clip-drag-identity-ui` で直交ドラッグ・原点復帰と保存を切り分ける。
  前者の `autosave`・`incremental autosave` は保存直後、`commit recovery` は作業ファイルを古くした後のコミット読出し、
  `history restore`・`restore undo`・`restore redo` は履歴復帰の各境界を示す。失敗名から formatter・増分キャッシュ・復帰を分けて調べる。
  正確な旧表記は維持し、損失がある時だけ往復可能な表記へ変える。既に丸められた値は復元しない
  （[ADR 0196](decisions/0196-lossless-package-floats.md)）。

- **パラメーターを含むパッケージの読込みが拒否される**:
  `track json:` の必須項目・型・未知の項目・`invalid id`・`unknown parameter` は `Parameter` の形の復号で調べる。
  `outside its range` は組立て後の制御の順序・端点・既定値と値の検証であり、復号の成功はプロジェクトの有効性を保証しない。
  `parameter_package.rs` は各段階の拒否と原本の保持、旧スキーマ・保存のキー順とコミットからの再読込みを検査する
  （[ADR 0222](decisions/0222-parameter-package-fields.md)）。

- **デバイスを含むパッケージの読込みが拒否される**:
  `track json:` の必須項目・型・未知の項目・`invalid id`・`unknown device kind` は `Device` の形の復号で調べる。
  `mixer state must be null` と `does not match its plugin id` は履歴の組立て前の検査である。
  状態オブジェクトの欠落・`does not match its BLAKE3` は保管庫の検査で、モデルの直接復号は代わりにならない。
  ピン・フラグ・順序・ID の重複は組立て後の不変条件と区別する。
  `device_package.rs` で拒否と原本保持、旧版・保存のキー順と改行・コミット再読込みを検査する
  （[ADR 0223](decisions/0223-device-package-fields.md)）。

- **ノートを含むパッケージの読込みが拒否される**:
  `content json:` の必須項目・型・未知の項目・`invalid id` は `Note` の形の復号で調べる。
  `articulation is not accepted yet` と `expressions are not accepted yet` は組立て前の未対応機能の拒否である。
  `not usable`・`not in start, pitch, id order`・`id is already used` はプロジェクトの組立てで調べる。
  直接の Serde は任意の整数・参照を表せても、音高・長さ・順序・ID の一意性を保証しない。
  `note_package.rs` は形と意味の拒否・原本保持、保存バイト・旧版・共有内容・コミット再読込みを分けて検査する
  （[ADR 0224](decisions/0224-note-package-fields.md)）。

- **トラックを含むパッケージの読込みが拒否される**:
  `track json:` の必須項目・型・未知の項目・親の `invalid id` は `TrackMetadata` の形の復号で調べる。
  `does not contain that id` はファイル名と同一性の照合、子の ID・中身・ミキサーの拒否は子の組立てと区別する。
  `track is not in the project`・`cannot contain itself`・`track order key` は親・階層・兄弟順の検証である。
  メタデータの直接復号や子との結合だけでは有効なプロジェクトにならない。
  `track_package.rs` は形・同一性・子と階層の拒否、原本保持・保存バイト・旧版・部分木の履歴を検査する
  （[ADR 0225](decisions/0225-track-package-fields.md)）。

- **項目表の変更後に保存・ジャーナル・スナップショットが一致しない**:
  モデルの `*_fields.rs` と投影指定、`changeset.rs` のプロパティ名、`change_values.rs` の値の codec を確認する。
  保存の未知の項目・型は `project.json:` / `content json:` / `track json:` の形の復号で拒否する。
  配置の不正 ID・レーン・種類も `track json:` の文脈を持ち、組立て後の範囲・参照の拒否と区別する。
  履歴の `schema_fields.rs` と `change_value_wire.rs` は保存バイト・旧 wire 表記・異常入力と原本保持を検査する。
  ABI の値だけが違う場合は FFI の `snapshot_fields_tests.rs` と `Snapshot::source_track_index`、元ハンドルの寿命を確認する。
  子の解決・不変条件の検証を迂回して値の一致だけを合格にしない。新しいログや収集先はない
  （[ADR 0226](decisions/0226-model-field-registry-completion.md)）。

- **Option ドラッグで音高が動く・ベロシティが引き継がれない**:
  押下時の `PianoVelocityGesture` と通常の `PianoGesture` を分ける。横移動・グリッド・Shift はベロシティへ影響しない。
  `piano-note-velocity-ui` は解放位置と単一記録、`piano-note-velocity-cancel-ui` は取消後の遅れた解放を検査する。
  `note-velocity-model` は共有内容と配置窓を確認する。0 は無音であり、音源なしの無音と区別する。
  記憶は `ProjectController.noteVelocity` の 7 bit 値で、Undo で戻さず再オープン時に 100 へ戻すのが仕様である。
  実際のノートの保存値は 16 bit なので、表示との比較には上位 7 bit を使う。
  応答後だけ値が古くなる時は、記憶の `generation` と選択の順序を確認する
  （[ADR 0173](decisions/0173-piano-note-velocity.md)）。

- **下部の棒がノート追加になる・音域外で消える・値が食い違う**:
  棒は `PianoLayout.velocity`、ノートは `grid` と入力・描画の境界を分ける。
  `PianoFrame.velocityNotes` は時刻内の全音高を取り、開始が左端より前の継続音には棒を作らない。
  頭と棒の矩形は `PianoVelocityLane` を描画・ヒット・アクセシビリティで共有する。
  `piano-velocity-lane-ui` は実入力と履歴・保存、`piano-velocity-lane-frame-ui` は問い合わせ・仮表示・読み上げの値、
  `piano-velocity-lane-cancel-ui` は取消後の解放と空白クリックを切り分ける。
  `spike-piano-frame` の `checkedVelocityBars` と `mismatches` は倍率 1・2 の 0・64・127 の GPU 画素を含む。
  実画面と聴感はユーザー確認とする（[ADR 0175](decisions/0175-piano-velocity-lane.md)）。

- **ピアノロールの空白ドラッグで追加されない・開始位置が動く・取消後に追加される**:
  `PianoInsertion` の押下時の開始・カメラと、解放時の終端・音高を区別する。
  開始の吸着は押下時、終端の吸着は解放時の Shift を使い、最短長は新規ドラッグだけグリッド幅である。
  単クリックも記憶長で追加し、ダブルクリックの 2 回目は重ねない。3 pt 未満の移動は押下時の値を保つ。
  `piano-note-insert-ui` は実 View・製品ブリッジ・単一 Undo と保存、`piano-note-insert-cancel-ui` は下書きの破棄を切り分ける。
  仮表示はモデル ID のない `PianoRenderInput.insertion` であり、確定ノートや VoiceOver 要素へ混ぜない。
  古い描画・異なる配置での押下は受けず、文書・トラック・カメラの変更と終了後の解放も追加しない
  （[ADR 0171](decisions/0171-piano-blank-drag-insertion.md)）。

- **ズームでノートの吸着幅が変わる・固定幅が変わる・線と吸着が一致しない**:
  `PianoCamera.gridMode` と実効 `grid` を区別する。自動は横倍率で変わり、固定は音価を保つ。
  細かい固定幅の線は 4 pt 未満で間引くが、吸着幅と記憶済み入力長は変えない。
  `PianoFrame.gridTicks` は吸着由来の間引いた細分線、`gridLineTicks` は独立した拍・4 拍の目印を含む描画順の列である。
  目印の位置へ吸着しないのは正常。粗い幅で目印が欠ける場合は `piano-time-guides-ui`、
  GPU の実画素は `checkedIndependentGuides` と `mismatches` で分ける（[ADR 0187](decisions/0187-piano-independent-time-guides.md)）。
  `piano-auto-grid-ui`・`piano-fixed-grid-ui` で実入力・旧押下の取消・文書所有確認を分けて調べる。
  三連符・付点は `piano-rhythmic-grid-ui` で候補・音価表示と共通幅を検査する。
  8 分三連符は 235,200 tick、付点 8 分は 529,200 tick。表示名を tick の逆数で作ると誤った分母になる。
  トラック切替と再生成でモードが戻る場合は共通カメラと `onGridChange` の操作子反映を確認する。
  `spike-piano-frame` の `checkedGridLines` と `mismatches` は倍率 1・2 の通常・三連符・付点の細分線の GPU 画素を含む
  （[ADR 0185](decisions/0185-piano-auto-and-fixed-grid.md)）。実画面の密度と VoiceOver はユーザー確認とする。
  `grid pixel mismatch` は倍率・グリッド種別・tick・採取 x・実際と期待の色を示す。採取は描画と同じ Float 座標の被覆ピクセル中心で行う。

- **入力長が別トラックへ伝わらない・Undo で変わらない・再読込後に変わる**:
  `ProjectController.noteInput.length` が未指定なら現在のグリッド幅を使う。再オープンで未指定へ戻るのは正常である。
  記憶後のクリックは短い長さも丸めず、ドラッグだけ新規のグリッド最短長を使う。
  `piano-note-input-length-ui` で選択・伸縮・別トラック入力と Undo / Redo、`NoteInputHistoryTests` で履歴枠と応答順を区別する。
  ノート以外の編集の記録も `recordEdit` を経由し、選択や背景保存では枠を増やしていないか確認する
  （[ADR 0172](decisions/0172-piano-click-and-input-length.md)）。

- **ピアノロールで延ばしたノートがクリップ端で切れる・移動すると消える**:
  ノート自体の `start` / `length` と配置の `start` / `length` / `offset` / `kind` を分けて確認する。
  小節未満の移動・複製だけでは自動配置を固定にしない。手動トリムと旧ジャーナルの移動後は固定なので区別する。
  `piano-note-clip-bounds-ui` は実 View・controller・ABI・確定フレームと Undo / Redo・保存を通す。
  自動窓は `origin = start - offset` から数えた中身の小節に合わせる（[ADR 0170](decisions/0170-content-relative-auto-bars.md)）。
  移動後にタイムラインの小節へ戻る場合は `placement.rs::refit_one` と `check_invariants` の基準を照合する。
  `clip-content-bars` は編集・挿入・削除と履歴・保存、`journal-clip-bar-basis` は印のない旧操作の固定化を切り分ける。
  固定窓の時刻編集での拡張は `placement.rs::grow_edited_window`、自動窓の結合は `refit_auto` を見る。
  左側だけずれる場合は、共有内容の結合による参照先更新より後に開始とオフセットを一緒に動かしているかを確認する。
  他の固定リンクの窓を保つこと、重複する拡張を編集全体で拒否することは仕様である
  （[ADR 0168](decisions/0168-note-edit-clip-growth.md)）。実画面の見え方・聴感はユーザー確認とする。

- **ピアノロールのノートが Shift でも吸着する・短くできない・離すと位置がずれる**:
  `PianoGridView.notePreview` が各ドラッグと解放の Shift を `PianoGesture.preview` へ渡しているかを確認する。
  最短長は通常グリッド幅、Shift では 1 tick。左右端の固定側と、移動の長さは変えない。
  `piano-note-drag-ui` は実 View の入力・単一 Undo・保存を、`PianoDragTests` の境界回帰は幾何を切り分ける。
  解放時だけ変わる場合は、最後のプレビューをそのまま確定せず、解放時の座標・修飾キーで計算し直しているかを見る
  （[ADR 0167](decisions/0167-piano-note-drag-snapping.md)）。実画面の操作感はユーザー確認とする。

- **アレンジのノートが帯のまま・音高が逆・VoiceOver と高さが違う**:
  中身の `PitchRange` は表示中のノートでなく全ノートから求める。スクロールやトリムしたリンク配置で縮尺が変わるなら索引の範囲を確認する。
  `arrange-note-pitch` は音域と 1 pt 下限、`arrange-note-pitch-ui` は製品の Metal と VoiceOver 用の局所矩形を切り分ける。
  `ArrangeFrame.instance` でノートの高さから行区切りの 1 pt を引いていないか、`accessibilityRect(for:)` が最後に提出したフレームを使うかを見る。
  問い合わせだけの時間は `spike-arrange-query`、実画面の提出・見え方は S1 とユーザー確認で区別する。
  `make debug-report` は検査の `target/verification/spike-arrange-query.log` も収集する。
- **開始位置の変更・ループ・メトロノームで、ボタンやルーラーが一瞬暗転する**:
  トランスポートの欄・ルーラー・コマンドは `ProjectController.transportLocked`（文書なし、または `busy`）だけで無効になる。
  `audioBusy` は応答待ちの有無であり、操作子の有効状態に使っていないかを確認する。
  続けた操作が反映されない時は、`orderedAudioCommand` の順番待ちと、トグルが番の来た時の状態を読んでいるかを見る
  （[ADR 0160](decisions/0160-ordered-transport-requests.md)、必須契約 `transport-order-ui`）。
- **FX 候補のダブルクリックで追加されない・一覧が重い**:
  一覧は `PluginListTable` で、`mouseDown` の `clickCount == 2` が行のどこでも追加する。
  追加されない時は、入力先トラック（`capturePluginInsertion` が nil）と文書の交換中（`busy`）を区別する。再走査中は追加を止めない
  （[ADR 0162](decisions/0162-plugin-candidate-table.md)、必須契約 `plugin-list-ui`）。
- **置いた音声の頭や終わりでクリック音がする・フェードのハンドルが押せない**:
  取込みはフェード 0 で置く（[ADR 0161](decisions/0161-placed-audio-without-fades.md)）。素材の端の不連続であり、再生の不具合ではない。
  ハンドルはクリップの端から 4 pt 内側に表示する。押せない時は `ClipControlLayout.entering` / `leaving` とクリップの矩形を比べる。
  以前のジャーナルの `audio.place` は 5 ms で再生する（必須契約 `journal-place-fades`）。

- **素材の終端後に波形が続く・テンポ変更で波形の位置がずれる**:
  音の変化と波形だけの誤表示を区別し、`Waveforms::fill` の問い合わせ範囲・素材フレーム数・テンポ点を残す。
  `audit_cur02_waveform_keeps_silence_after_the_source_ends` と
  `audit_cur03_waveform_maps_each_column_through_the_tempo_map` が、列ごとの時間対応を通常回帰で検査する。
  `waveform_partial_views_keep_multiple_tempo_changes_and_trim_offsets` で全幅・部分幅・キャッシュ再読込みを比較する。
  EOF 超過と途中のテンポ変化は独立した条件である（[CUR-02 / 03](reviews/2026-10-25-code-audit.md)）。
- **API / MCP の同値設定で Undo が何も戻さない・Redo が消える**:
  無変更は成功として Action／配列を返すが、新しい Undo を意味しない（[ADR 0182](decisions/0182-noop-edits.md)）。
  `Session` の空差分判定と束の正味の変更、`DocumentHost` のジャーナルの増加判定を確認する。
  `noop_edits.rs` と MCP の `contract_mcp_noop_edits_succeed_without_consuming_history_or_redo` で切り分ける。
  画面だけ件数が増える時は、perform 完了の `historyReport.changed` と `recordEdit` の条件を照合する。
  旧版の `changes: []` の行は自動削除しない。同値の依頼・前後の連番・Redo の有無を記録し、原本を保持する。
- **API / MCP の呼出しが返らず、後続クライアントも止まる**:
  ソケットの読込み待ちだけでなく、応答の書込み待ちを確認する。
  `serve_connection_limited` は読書きの無進行を 30 秒で打ち切る。
  `could not write API reply` は応答送信の失敗であり、編集そのものの失敗とは限らない。
  `audit_cur04_an_unread_reply_releases_the_connection` は送信バッファを満たし、後続接続と編集の単一実行を通常回帰で確認する。
  編集が既に記録された可能性があるので、同じ編集を無条件に再送しない（[CUR-04](reviews/2026-10-25-code-audit.md#cur-04-api-の書込みにはタイムアウトがない)）。
- **開き直すと確定済みの編集が消えている・古い状態で開く・開けない**:
  警告または C ABI の `historyReport.recovery` の `tip`・`base`・`applied_seq` で、先端からの後退を確認する。
  `DocumentHost::recovered_label` は従来どおり、オープン時の基点と復旧済み連番を返す。
  先端が読めない原因は、オブジェクトの破損と、不変条件を破ったモデルのコミット（読込みで拒否される）を区別する。
  破損ジャーナルは `journal seq <連番> violates model invariants: <理由>` でオープンを拒否する（[ADR 0180](decisions/0180-journal-replay-invariants.md)）。
  `audit_adv01_*` は通常回帰で、先端なし・ありと、明示修復後に確定した編集の保持を検査する。
  原本をコピーして保全し、連番・不変条件の理由・基点と先端を記録する。自動で行を削除したり、繰り返し保存して直そうとしない。
  旧版では不正な状態をコミットし、debug では最初の編集が panic する経路もあった。
  後退したホストは `recovery confirmation required` で書込みを拒否する（[ADR 0183](decisions/0183-recovery-consent.md)）。
  警告でいったんキャンセルし、原本のコピーを保全する。「続行」は復旧状態の同期保存であり、失われた編集の復元ではない。
  確認時の保存に失敗した場合は編集を解放しない。CLI / MCP には同意の入口がないため、アプリで確認する。
  確認の取消後も開く処理が残る場合は、要求の UUID と `RecoveryOpenRequest` の取消・`recoveryReply` の解放を調べる。

- **下部を開くと別のエディタが消える・高さが戻らない**:
  ⌥⌘3・4 は各エディタ、⌥⌘5 はアレンジ非表示の独立した切替である。
  `workspace.layout.v2` の `editor`・`devices`・`editorsMaximized` と `editorHeight` を分けて確認する。
  v1 の切替は保存されていないため、移行時にデバイスの開閉を推測しない。
  壊れた v2 は既定値へ戻すので、古い v1 の削除を初手の対処にしない。
  `workspace-layout-ui` は共通コマンドと設定復元、`workspace-stack-ui` は画面なしの製品の枠・順序を検査する。
  両方開いた通常表示の最小高は 870 pt、最大化は 645 pt で、最小高の変化を高さ設定の喪失と混同しない
  （[ADR 0146](decisions/0146-independent-editor-stack.md)）。
- **ヘッダの名前が出ない・名前で候補が開く・違う窓が開く**:
  名前は行高 52 pt 以上かつ幅と字下げに余裕がある時に表示する。先頭以外は「…」のメニューで選ぶ。
  `TrackFXNamesLayout` の矩形とカタログ名、スナップショットのチェイン順を確認する。
  名前は `onOpenPlugin`、追加ボタンは `onOpenFX` であり、`fx-header-ui` で入力と古い対象の拒否を区別する。
  その後のヘルパーの表示失敗は既存の `plugin-window-helper` の経路を調べる（[ADR 0139](decisions/0139-track-header-fx-names.md)）。
- **窓アイコンが消えない・閉じた窓が再び開として表示される**:
  アイコンは物理的な可視性ではなく寿命を示す。別アプリの操作中に隠れていても開の表示は正しい。
  `plugin-window-state` でソケット受信順の `window_state`、`plugin-window-status-ui` で画面の `openPluginWindows` を確認する。
  活動の `window` は編集時の過去の文脈、`closed` は未読期間内の閉鎖の有無であり、現在の表示へ代用しない。
  開く要求の返答から楽観的に表示を更新していないか、未読圧縮で最後の寿命を保っているかも調べる。
  削除・オフライン後の残留は `publish`、文書交換後の残留は `DocumentSession.retire` と、処理が保持するセッションを確認する
  （[ADR 0138](decisions/0138-plugin-window-status.md)）。
- **デバイスの一覧が隠れる・折りたたみ後の FX ドロップ先がずれる**:
  名前の左の開閉状態を確認する。折りたたみは音やオフライン状態を変えず、検索・一覧・状態の本文だけを隠す。
  展開しても一覧が戻らない時はパラメーターの取得失敗と区別する。
  `device-disclosure-ui` で可変幅の列の中央前後と送り先 ID を確認し、配置・挿入先に固定の列幅を使っていないか調べる。
  トラック切替・エディタ再生成で開閉が戻るなら `collapsedDevices`、文書交換後も残るなら、書込み先が `controller.session` と同じセッションかを確認する。
  見えない検索欄がキーを消費する時は折りたたみ時のフォーカス解除を確認する（[ADR 0137](decisions/0137-device-disclosure.md)）。
- **値の操作子のクリックで値が跳ぶ・ダブルクリックが記録されない・入力欄が閉じない**:
  規則は `ValueClicks` の段階（押下・ドラッグ・既定値の表示中・3 回目待ち）で決まる。単クリックで値が変わるなら、
  NSSlider の追跡や絶対位置の計算が戻っていないかを `track-mix-ui`・`fx-params-clicks-ui` で確認する。
  既定値が記録されないなら、待ち時間（`NSEvent.doubleClickInterval`）の前に文書が変わって下書きが消えたか、
  プレビューの開始（`beginFader`・`beginParameter`）が拒否されたかを区別する。3 回目で既定値が残るなら `mix-clicks-ui` を使う。
  別の操作子へ移ると拒否される場合は、待ち対象と次の押下対象、先行プレビューの終了を区別する。
  `if case .waiting(target)` は既存の対象値を照合している。全待ち状態へ広げて旧押下を流用しない。
  `mixer-reset-target-ui`・`clip-reset-target-ui` は対象切替と個別の履歴・保存を検査するが、編集完了前の連続入力や実機のクリック間隔の保証ではない。
  入力欄が赤いまま閉じないのは、範囲外・単位違い・プラグインの拒否（`converts` が真で `value` が空）である。
  変換を持たないプラグイン（`converts` が偽）は数値だけを受ける。`helper did not read the parameter text` は通信の失敗で、
  `plugin-text-parse` で実ヘルパーの `text_to_value` を切り分ける。
  入力中の Space で再生が始まる時は、欄の `performKeyEquivalent` が先に受けているか、欄が最初のレスポンダーかを確認する
  （[ADR 0153](decisions/0153-value-control-clicks-and-typed-entry.md)）。
- **パラメーター・バイパス・オフライン操作で全ボタンが一瞬暗転する**:
  `ProjectController.busy` の完了時の値だけでは短い暗転を見落とす。`device-edit-enabled-ui` で操作中の全遷移を観測する。
  `EditTurn.busy` は編集待ち行列の状態、画面の `busy` は作成・交換・終了の明示的な排他だけである。
  排他の既定値を通常編集へ広げていないか、各ビューが個別の `historyBusy` と全体の `busy` を取り違えていないか確認する。
  FX 追加での素材・保存済み状態の読込みや、同一ヘルパーの状態復元を待つ経路とは別である（[ADR 0133](decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
- **プラグイン付きトラックの選択が止まる・パラメーター画面が重い**:
  2026-10-20 のユーザー報告は「選択のたび」と「汎用パラメーター画面」の 2 症状であり、独自プラグイン GUI の問題ではない。
  機器なしで、旧欄の全行一括生成（800 個で約 0.68 秒）と再選択での作り直し、文書ワーカー上の応答待ちを再現し、対応した
  （[ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)）。
  再発時は、プラグイン名・版・パラメータ数、再生状態、下部の表示、初回と再選択、停止時間、入力全体が固まるか操作が無効表示になるかを区別する。
  初回だけ一覧が遅いなら、音声状態の `opening` を見る。真の間に編集・Undo が終わるなら、出力の準備（ヘルパー起動・素材）の時間である。
  未取得の列の「音声出力を準備中」は同じ `opening` の表示であり、個別プラグインの進捗率ではない。
  出力の準備が終わっても一覧が来なければ「パラメーターを読み込み中」へ戻る。失敗とオフラインは待機表示より優先する。
  状態と列の表示が食い違う場合は `device-preparation-ui` と `DevicePreparationTests` を使う（[ADR 0141](decisions/0141-device-output-preparation-status.md)）。
  同テストの期限失敗は、未保留デバイスの返答・初回の音声状態・準備後の音声状態のどの待ちかをメッセージで区別する。
  サニタイザーの失敗ログを保持して単独実行と全体実行を比べ、期限だけを延長して製品不良や並行実行の影響を隠さない。
  `opening` の間に編集も終わらないなら、ワーカーが準備を待っている。`output-opening-worker` を実行して手番を確かめる。
  導入後に位置・ループ・追加したトラックが反映されないなら `output-opening` で導入時の差分を確かめる（[ADR 0136](decisions/0136-output-opening-off-worker.md)）。
  開いた出力で FX を追加した時は `pending_plan` を見る。新しいヘルパーの初期化と共有領域の準備は `echode-graph-prepare` が待つ。
  その間の編集と停止は `background-graph`、導入後のデバイス要求の待ちは Session の機器なし回帰で区別する。
  状態の復元やループ変更の後で既存ヘルパーが再生成されるなら、投入時の状態一致の保持と次の `Pending` の経路を確認する。
  `opening` が偽でも `pending_plan` は真になり得る。置換専用の列の表示は未接続である。
  素材・保存済み状態の読込みや初回導入時の差分は同期のままなので、ヘルパー初期化の待ちと分ける
  （[ADR 0142](decisions/0142-background-plugin-graph-preparation.md)）。
  機器なしの出力は計画の採用に検査側の `pump` が必要である。初回の準備だけを待つ時に `Session.opening()` の全置換待ちを使わない。
  製品アプリでは開いた直後の `opening` は事前準備（`audio.prepare`）である。最初の再生がまだ待つなら、押した時点の状態を分ける。
  `opening` が真なら事前準備の途中、偽で `pending_plan` が真なら停止中の編集の素材読込み（`echode-media-prepare`）の残りである。
  `playing` が真で `running` が偽のまま止まるなら、読込みの失敗か保留の解除を `output-prewarm-play` で確かめる。
  開いても `opening` にならないなら、`output-prepare-on-open` で controller の送信、`output-prepare-worker` で C ABI を確かめる
  （[ADR 0147](decisions/0147-output-prewarm-and-background-media.md)）。
  毎回止まるなら、`DeviceParameterListView.visibleRows` の数と `table.numberOfRows` を比べ、全行を作っていないかを確認する。
  列が作り直されているなら `DeviceStripView` の列の保持（文書の切替でだけ捨てる）を確認する。
  値の往復で止まるなら、`device-lane-held` を実行し、背景保存の確認と適用済み状態の読取りが通信中の制御端点を待っていないか確かめる。
  画面だけが古い一覧へ戻るなら、`device-view-slow-ui` で要求世代の照合を確かめる。
  テスト用 CLAP の `ECHODE_TEST_FAULT=params-wait:<socket>` は一覧をメインスレッドで保留し、機器なしで遅いプラグインを再現する。
  実測時間を使う性能検査は通常回帰から分ける。実機操作とサンプリングが必要な場合は担当・許可を確認し、既存アプリの終了・置換は行わない。
  [追加受入条件](phase2.md#p2-02-デバイスビューとパラメーター編集着手中)を参照する。
- **プラグイン GUI がすぐ消える・ループ切替で状態が戻る・Space が届かない**:
  `plugin-window-visibility` は OS の一時非表示を閉鎖と誤判定する経路を確認する。
  `NSPanel.close` の記録、親・自分・同じ親のヘルパーの最前面判定を調べ、実際の閉鎖と隠れただけの状態を区別する。
  `plugin-window-loop` は状態の取り込み後に未保存の値を変え、停止中・再生中のループ切替で同じ GUI と値が残ることを確認する。
  `Pending.restore_states` がループだけで真にならないこと、保留中のプロジェクト編集の復元を落とさないことを調べる。
  キーは `plugin-key-events`（AppKit）、`plugin-key-socket`（制御通知）、`plugin-key-ui`（既存コマンド）で境界を分ける。
  ヘルパーはコマンドを名前（`PluginCommand::name`）で送り、画面は `WorkspaceCommand` の `rawValue` で受ける。
  コマンド名を変えて片方だけ直すと、中継したキーは黙って無視される。`plugin-key-ui` は未知の名前をその場で失敗にする。
  以前はここで来ない音声コマンドを待ち続け、Swift の全体実行が 1 件のテストで止まった（[ADR 0155](decisions/0155-remembered-playback-start.md)）。
  文字入力中は転送しない。独自入力欄は `NSTextInputClient.selectedRange` の公開状態を確認する。
  実画面の再発時はプラグイン名・版、前面のアプリ、窓を開く前の状態保存、入力欄の編集中かも記録する
  （[ADR 0134](decisions/0134-plugin-window-lifetime-and-shortcuts.md)）。
- **プラグインのウィンドウが開かない・フォーカスを奪う・値や Undo が届かない**:
  エラー表示で境界を分ける。`the plugin has no window` はプラグインが `clap.gui` を持たない、
  `the plugin has no Cocoa window` は Cocoa の API を持たない、`the plugin refused the window` は `set_parent` の拒否である。
  `plugin window timed out` はヘルパーのメインスレッドが 8 秒以内に `create` / `show` を終えなかったことを示す（[ADR 0132](decisions/0132-helper-plugin-windows.md)）。
  開くまでの待ちは、停止中の最初の表示で出力とヘルパーを準備する待ち（`opening`）と区別する。
  ウィンドウがキーになる・隠れない・戻らない場合は、記録用の窓で規則を確かめる `plugin-window-helper` を先に実行し、
  合格なら AppKit の実画面の問題としてユーザーの確認へ回す。実画面の操作は依頼がある時だけ行う。
  値が一覧へ届かないなら `plugin-window-relay` で処理スレッドのリングと制御スレッドの読み戻しを、
  Undo が増えない・増えすぎるなら `plugin-window-undo-ui` でジェスチャーの数え方と静止の待ちを確かめる。
  窓が閉じている時のプラグイン自身の変化は自動保存だけで、Undo に入らないのが正しい。
  テスト用 CLAP の `ECHODE_TEST_FAULT=gui:<socket>` は、ヘルパーに記録用の窓を選ばせ、ソケットからプラグイン画面の操作を送る。
- **起動直後から新規作成・開くが無効で「変更を保存中」のまま**:
  前回プロジェクトの自動復元と `ProjectController.busy` を確認する。
  `sample` の読取で復元側が `Autosave::drop` の join、`echode-autosave` が初期化失敗後の受信待ちなら、終了要求を無視する旧不具合と一致する。
  初期化失敗中も `Shutdown` で終了し、復元エラーとパッケージロックの解放を返す必要がある。
  `startup-recovery` は子プロセスの停止防止期限でこの循環待ちを検出し、Swift の `failedStartupOpenReenablesNewAndOpen` はエラー後の新規作成を確認する。
  復元対象の削除や前回パスの初期化で問題を隠さず、エラーと Git の状態を切り分ける。
- **直したのに画面が変わらない**:
  `doctor` でアプリが `stale` / `unknown` なら `make app` を実行し、`target/Echode.app` を開き直す。
- **FX が違うトラックへ入る・並びが変わらない**:
  候補を開いた文書セッション、追加操作時の選択トラック、ドロップ先トラックを区別し、`device.reorder` / `device.copy` と状態ハッシュの引き継ぎを `fx-chain-ui` で確認する（[ADR 0118](decisions/0118-track-fx-chain-editing.md)）。
  インスペクターは `WorkspaceInspectorView` → `InspectorAddFX` → `openPlugins(on:)` の順で、表示した文書とトラックを保持する。
  `fx-inspector-ui` は実ボタンから空のチェイン・対象の選択変更・旧文書を確認する。名前クリックによる独自 GUI の入口とは別である。
- **トラックが違う位置へ追加される・追加後の選択が違う**:
  `TrackInsertionRequest` の文書セッションと `after`、ジャーナルの `track.add` / `track.insert` と `into_folder` を区別する。
  展開フォルダは最後の子、折りたたみ中は部分木の外側、末尾ボタンは最上位末尾へ追加する。
  `TrackInsertionTests` と `track_insertion` のログで操作後の選択変更・消えた対象・旧文書の拒否を確認する
  （[ADR 0127](decisions/0127-positioned-track-insertion.md)）。
- **複製が違う位置に入る・二重に増える・元のノートまで変わる**:
  `TrackDuplicationRequest` の文書セッションと `sources`（選択された祖先を持つ行は除く）、ジャーナルの `track.duplicate` の `source` と `seed` を区別する。
  複製の ID は `seed` から導出した UUIDv8 で、元と同じ中身 ID の配置が複製先に残っていれば独立化の不具合である。
  `TrackDuplicationTests` と履歴・CLI の `track_duplication` で、位置・選択・中身の独立・書出しの 2 倍一致を確認する
  （[ADR 0129](decisions/0129-track-duplication.md)）。
- **クリップが違う位置・トラックへ移る・重なった配置が消える・複製の編集が元へ伝わる**:
  `ClipMove` の文書・配置・トラック・開始・`copy` と、ジャーナルの `clip.move` / `clip.duplicate` の `seed` を区別する。
  開始は押下時のポインターの拍からの移動量で決まり、四分音符へ吸着、Shift で解除、Command は離した時点の値を使う。
  重なる配置の削除・切詰め・分割は置き換えの仕様で、分割の右側は `seed` から導出した ID を持つ。
  複製の配置が元と同じ中身 ID なら独立化の不具合である。
  `ClipMoveTests` と履歴・CLI の `clip_move` で、吸着・選択・置き換え・書出しの一致を確認する
  （[ADR 0154](decisions/0154-clip-move-and-duplicate.md)）。
  単一クリップの ⌘D は `ClipDuplicationTests`・`ClipDuplicationBoundaryTests` で、現在の窓の終端・整数上端・文書・入力フォーカスを区別する。
  文字入力中・別ペイン・未確定ジェスチャー・キーの押し続けで複製しないのは仕様である。
  `ArrangeView.performKeyEquivalent` と `keyDown`、`ClipMoveTarget.adjacentStart`、送信後の `clipFocus` を確認する
  （[ADR 0188](decisions/0188-arrange-adjacent-clip-duplication.md)）。
- **フォルダ開閉後に行・ミックス値・FX の対象がずれる**:
  元スナップショットと表示用ハンドルの行数・ID、`TrackTreeInfo` と `row_map` を区別する。
  ヘッダ、可視矩形、ミキサーとデバイスが同じ表示行から元トラックを参照することを
  `FolderDisclosureTests` と `snapshot_ownership` の必須契約で確認する。
  開閉後に古いフレームが反映される場合は、入力ハンドルと描画キャッシュの版も確認する
  （[ADR 0128](decisions/0128-folder-disclosure-and-row-projection.md)）。
- **FX の一括追加が一部だけ・Undo が複数回になる**:
  単一フォーカスと `selectedTracks`、クリック時の `PluginInsertionRequest` の文書・対象配列を区別する。
  製品 ABI が配列を同じ `DocumentHost::apply` へ渡すことを `TransactionTests` と `fx-batch-ui` のログで確認する。
  選択変更後の付け替え、旧文書、欠けた対象、途中失敗時の部分追加を分けて調べる（[ADR 0124](decisions/0124-selected-track-fx-batches.md)）。
- **サンプル検索で見つからない・配置先や位置が違う**:
  登録フォルダの一覧と索引の読込中表示、読込失敗の詳細を確認し、素材変更後は更新ボタンで再読込する。
  索引は隠しファイル・シンボリックリンク・パッケージ内部を含めず、重なる登録から同じパスを重複表示しない。
  配置は `AudioImportRequest` の文書セッション・トラック・tick を使う。
  定期更新の `audio.tick` とライブの `playback.position.tick` を区別し、非同期待機後の選択へ付け替えない。
  `SampleBrowserTests` と必須契約の検索・配置ログを確認する（[ADR 0122](decisions/0122-sample-library-search-and-placement.md)）。
- **音声をドロップできない・別の行や位置へ入る・トラックだけ残る**:
  対応音声のファイル URL 1 つと copy 操作だけを受ける。MIDI・複数 URL・ヘッダ・分割線・領域外は拒否する。
  `sample-browser-content-ui` は表示と URL の書出し、`sample-drop-entry-ui` は AppKit の入口と文書交換を切り分ける。
  `sample-drop-ui` で現在のヘッダ幅・カメラ・折りたたみ後の行・Shift を確認する。ダブルクリックの再生位置配置とは別の経路である。
  受理後の失敗は文書のエラーを確認し、復号・消失した対象・同トラックの配置重複を区別する。
  トラックだけが残る場合は `audio-import-transaction` と `DocumentHost::apply` の失敗した要素の番号を調べる。
  PCM と索引の残存は従来の仕様で、モデルの部分更新とは別である（[ADR 0145](decisions/0145-sample-browser-file-drops.md)）。
- **サンプルの試聴が鳴らない・古い素材が鳴る・止まらない**:
  スナップショットの音声状態 `audition` の `request`・`handled`・`phase`・`error`・`sample_rate` を見る。
  `handled` が `request` に届かなければ試聴スレッドが処理前、`failed` は復号か出力準備の失敗、`sample_rate` が空なら出力は閉じている。
  機器の通知を受けると出力を閉じ、`stopped` と機器変更のエラーを残す。
  ボイスは `echode-engine` の試聴の単体回帰、スレッドとコールバックは必須契約 `sample-audition` で切り分ける
  （[ADR 0135](decisions/0135-sample-audition.md)）。
- **試聴が遅れる・意図しない試聴が鳴る・クリックでフォーカスが移らない**:
  `make debug-report` の `app-logs/audition.ndjson`（アプリが書く要求ごとの記録、直近はスナップショットの `audition.history`）を読む。
  `input_delay_us` は入力イベントから要求まで（画面の待ち）、`picked_us` から `handed_us` は試聴スレッドの各段、
  `heard_us` はその音を採用したブロックの出力時刻である。`opened_output` が真なら出力を開き直している。
  `input` が `other` の要求、同じパスの繰り返し、`superseded` の多さで、利用者の操作以外の要求や連続した選択を区別する。
  `first_decoded_us` と `converted_us` は最初の塊、`decoded_us` は全体の復号であり、長い素材では受渡しより後になる。
  `waveform.loaded_frames` が止まり先読みが埋まっている間は正常な待ちである。
  初回の読込み中に出力が閉じた場合は、機器変更・失敗とアイドル終了を区別する。
  `sample-audition-loading` は読込み中のアイドル終了を拒否し、停止後の閉鎖も確認する。
  `starved_frames` の増加は読込みが再生に追い付かなかった出力フレーム数で、素材の無音や EOF とは異なる。
  必須契約 `sample-audition-stream` と機器なしの `spike-audition` で先頭の受渡しを切り分ける。
  波形のパスが選択と違う場合は `SamplePreviewView` のパス照合、波形の末尾は `complete` と元の `frames_per_peak` を調べる
  （[ADR 0151](decisions/0151-streaming-sample-audition.md)）。
  クリックの選択とフォーカスは必須契約 `sample-browser-click-ui` で確認する。
  `settings.visible` が真なら無音でも出力を保持し、`output_error` は素材の要求前の準備失敗を示す。
  `settings.gain` が 0 なら試聴だけが無音になる。自動試聴の設定は `workspace.audition.v1` を見る。
  履歴の `start_frame` は元サンプル位置、波形の `first_frame` と `loaded_frames` は読んだ範囲の先頭と末尾である。
  先頭以外からの `complete`・`decoded_us` は EOF までの読込み完了であり、未読の先頭部分の波形は含まない。
- **ブラウザの波形が出ない・停止で消える・古い形のまま**:
  一覧の下の波形は試聴の報告ではなく `SampleOverviews` の全体の読込みを先に使う（[ADR 0163](decisions/0163-sample-overview.md)）。
  選択の変化で `FileBrowserState.overviewTask` が始まったか、項目が `loading` / `ready` / `failed` のどれかを確認する。
  `loading` のまま戻らない時は、より新しい要求で `cancelled` になった読込みを同じファイルの選択で読み直しているかを見る。
  取消はブラウザごとのチャンネル（`SampleOverviews.channel`）の中だけで起こる。別のブラウザやテストが止めている場合はチャンネルの共有を疑う。
  更新したファイルが古い形のままなら、サイズと更新日時（`FileStamp`）が変わったかを確かめる。`URL.resourceValues` の値は URL ごとに残るため使わない。
  Rust 側は必須契約 `sample-overview`、表示と保持は `sample-overview-ui` で切り分ける。
- **サンプル一覧にフォーカスがある時に Space で再生しない**:
  一覧の `FileListTable.keyDown` が `onPlayback`（ワークスペースの「再生／停止」）を呼ぶかを、必須契約 `sample-list-space-ui` で確認する。
  呼んでも鳴らない時は `WorkspaceSession.enabled(.playback)`（シート表示中・文書交換中）と出力の状態を見る。
  メニュー側の `workspaceTextEntry` が一覧のフォーカスで残るかは未確認である（[照合](reviews/2026-10-25-user-feedback.md#fb-02-サンプル一覧での-space)）。
  表示保持・音量・位置と実操作子は必須契約 `sample-audition-resident`・`sample-audition-gain`・
  `sample-audition-position`・`sample-audition-controls-ui` で切り分ける（[ADR 0152](decisions/0152-browser-audition-controls.md)）。
  ブラウザ自身の選択の付け替えは要求を出さない（[ADR 0143](decisions/0143-browser-click-and-audition-timing.md)）。
- **選択トラックのミュート・ソロが部分適用される／連打で戻らない**:
  `MixerSwitchBatch` の文書・捕捉 ID 群・anchor・反転／絶対値を、操作後に変わった現在の選択と区別する。
  `mixerSwitchTail` と `EditTurn` の手番で反転元の現行値を確認し、`param.set` 配列を 1 回送っていることを追う。
  欠けた対象と旧文書は全体を拒否し、同じ絶対値は Redo を消さない。ピアノ右側の行ボタンは単一対象を保つ。
  切分けは `MixerSwitch*Tests` と必須契約 `mixer-switch-batch-ui`・`mixer-switch-order-ui`
  （[ADR 0189](decisions/0189-selected-track-mute-and-solo.md)）。
- **音量・パンの数値入力で選択全体が変わらない／入力値まで動かない**:
  `MixActions.enter` と `MixerValueBatch` の捕捉 ID 群・基準 ID・要求値を確認する。
  数値入力とヘッダの読み上げ増減が相対一括変更で、ドラッグ・既定値リセットは単一対象を保つ。
  `mixerSwitchTail` の手番で現行値を読み、最大音量 4・パン ±1 を最初に踏む対象で共通差分を止める。
  無音の基準からの音量増加は比が決まらないため理由を表示し、他の無音は有限倍率でも無音を保つ。
  古い文書・欠けた ID の全体拒否と no-op による Redo の保持を、`MixerValue*Tests` と
  `mixer-value-batch-ui`・`mixer-value-bounds-ui` で切り分ける（[ADR 0191](decisions/0191-selected-track-numeric-mix.md)）。
- **ヘッダの読み上げ増減で選択全体が動かない・段数が違う・読み上げ値が先走る**:
  `MixActions.adjust` と `captureMixerStep` の捕捉 ID 群・方向（±1）、`mixerSwitchTail` の手番の現行値を確認する。
  AppKit の主スレッドの呼出しは同期捕捉、別スレッドからは UI actor に渡す。木の更新前の絶対値を繰り返し送らない。
  共通限界で止まる値を `MixerStepBoundaryTests` で読み戻し、未確定の予測値を読上げ値に使っていないかを見る。
  複数対象の無音からの増加拒否と、有限差分では −60 dB 未満を保つ規則を、単一スライダーの無音への段と区別する。
  再オープン後の誤編集は `documentID` 変更時の `controlElements` の破棄と旧要素の文書照合を追う。
  切分けは `mixer-step-ui`・`mixer-step-bounds-ui` と `MixerStepOrderTests`
  （[ADR 0192](decisions/0192-selected-track-accessibility-steps.md)）。
- **文書を開き直した後に古いヘッダ操作で値が変わる／数値欄が残る**:
  `ArrangeHeaderView.documentID` の変更で `ValueClicks.cancel`・押下の破棄・欄の取消が走るかを確認する。
  `mixActions` の更新は文書 ID より先に来るため、終了は押下時の `gestureActions` を使い、新文書の別の試聴へ送らない。
  同文書の更新では押下を維持する。`mixer-gesture-document-ui`・`mixer-reset-document-ui` と
  `MixerGestureDocumentTests` で保存・履歴・callback の所有者を区別する（[ADR 0193](decisions/0193-header-mix-gesture-document-scope.md)）。
- **文書切替後に旧クリップのゲイン・フェード入力が反映される**:
  `ArrangeView.audioDocument` の変更時に `cancelClipControls` が押下・下書き・タイマー・数値欄を取り消すかを確認する。
  `setSnapshot` の下書き破棄だけでは旧押下と欄が残る。同じ配置 ID が再利用される再オープンでも文書 ID は異なる。
  開始時の `ClipPress.edit`・操作中の `gestureEdit` と現在の `onClipEdit` を区別し、旧操作へ最新の編集先を貸さない。
  投入済み数値入力は欄を閉じるだけでは止まらないため、実行時の文書照合も確認する。
  `clip-gesture-document-ui`・`clip-gesture-callback-ui` は実入力・製品 ABI・履歴と保存再読込を検査する
  （[ADR 0194](decisions/0194-clip-mix-gesture-document-scope.md)）。
- **縦だけのヘッダドラッグや原点での解放で音量が無音になる**:
  押下時の音量と横移動量を確認する。`MixScale.position` は −60 dB 未満を位置 0 へ切り詰めるため、
  横移動ゼロでは表示スケールを往復せず元の Float を返す。`mixer-drag-identity-ui` で試聴・Undo / Redo・再読込を確認する。
  非ゼロ移動の下限到達と区別する（[ADR 0153 の後続修正](decisions/0153-value-control-clicks-and-typed-entry.md)）。
- **画面のミックス値・メーターと音が一致しない**:
  確定スナップショットと一時的な `faderPreview`、`requested_plan` / `applied_plan` を確認する。
  メーターはフェーダー後の左右最大ピーク（停止中は 24 dB/s 減衰、−60 dB 以下で 0、無音表示は `−∞`）。VoiceOver の増減段数や数値欄の `−` 解釈は `MixControlTests` で確認する（[ADR 0107](decisions/0107-mix-controls.md)）。
- **ペインのドラッグ中に絵が伸縮する・狭くした後にヘッダ幅が戻らない**:
  `canvas-resize-ui` は両表示面の見た目の左上固定（反転した階層では AppKit が `.bottomLeft` を選ぶ）と即時の drawable サイズ、
  `header-width-ui` は幅 0・狭い幅の往復と表示記憶、`transport-layout-ui` は状態行と再生ボタンの寸法の不変を確認する。
  `CanvasSurface` がレイヤーのサイズを主スレッドで更新し、描画スレッドが後から古いサイズを書いていないか調べる。
  `ArrangeMeasurement.sizeMismatches` は入力と異なるサイズの drawable に現在の入力を描いた数で、リサイズ中の一時的な増加は正常である。
  安定後も増える場合は入力の幅・高さ・倍率と drawable のサイズを照合する。
  大きさの違う drawable は入力と同じ画素位置で描き、入力の外は `CanvasBackground` の背景色に保つ。
  行ラベルも入力の境界でクリップするため、下端の部分表示行が余白へ文字を出さない。
  `drawable-mismatch-ui` は製品と同じ `CanvasRenderer.submit` を使い、画素・描画済みの版・休止状態を確認する。
  不一致後に描き直されない時は、版を消費する条件と `publishLinkState` へ渡す不一致の状態を調べる。
  描画部品だけのオフスクリーン比較を、この制御経路の確認の代わりにしない。
  リサイズ中に暗い面や透明な余白が見える場合は、描画スレッドが古い大きさの入力で描いていないか、
  `CAMetalLayer.backgroundColor` が背景色と一致しているかを確認する（[ADR 0150](decisions/0150-stale-drawable-frame.md)）。
  `ArrangePaneView.headerWidth` と `CanvasNavigationState.headerWidth` は指定幅、描画入力の `header.width` は現在の制限値である。
  自動回帰は設定と幾何の確認であり、ドラッグ中の見え方は実画面のユーザー確認とする（[ADR 0144](decisions/0144-unscaled-canvas-resizing.md)）。
  絵が上下にずれる場合は、表示面の `layer.contentsAreFlipped()` と `contentsGravity` を照合する。
  反転した階層では CA の `top` が見た目の下を指し、AppKit は見た目の左上を `bottomLeft` に変換する。
  `contentsGravity` を `.topLeft` で上書きすると見た目の左下へ固定される（[ADR 0149](decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
  再生開始時だけずれる場合は、状態行などペイン外の高さの変化を疑う。状態行の表示器は準備中だけ見せ、場所は常に確保する
  （[GUI のがくつきの監査](reviews/2026-10-22-gui-jank-audit.md)）。
  高さを戻しても縦位置が戻らない場合は、`ArrangePaneView.viewport`（指定値）と
  `timeline.renderInput.camera.originTrack`（表示の制限値）を比べる。指定値が表示の制限で書き換わっていないかを見る。
  横スクロール・横ズーム・表示中のトラック選択で指定値が変わらないこと、上スクロールと縦ズームが表示中の行から始まることも見る
  （[ADR 0149](decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
  ピアノロールの表示やリサイズが重い場合は、`accessibilityBuilds` が描画のたびに増えていないかを見る。
  要素は問い合わせ時にだけ作り、同じ描画版では再利用する。再利用時も画面座標と選択値は更新される（同 ADR）。
- **プレイヘッド・リフレッシュレート・ペイン境界・行境界**:
  プレイヘッドは音声が最後に書いた tick を補間せず描画する。`displayFrameRateRange`（最大 120 Hz）、ペイン縮小時の `clipsToBounds`（[ADR 0098](decisions/0098-pane-render-boundaries.md)）、オフスクリーン検査の左余白比較点（`x=2 pt`）を確認する（[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)）。
- **キー入力・ホイール・検索・ループ欄**:
  ヘッダの `keyDown` から上位への Space 転送、マウスホイールの行単位とトラックパッドのポイント単位（[ADR 0050](decisions/0050-arrange-viewport.md)）、ピアノロールのドラッグ中のみの Escape と連続矢印キー（[ADR 0052](decisions/0052-piano-keyboard.md)）、空選択時の検索解除（[ADR 0053](decisions/0053-workspace-ui-foundation.md)）、ループ欄の確定時送信（[ADR 0055](decisions/0055-loop-metronome.md)）を確認する。
- **保存・復旧・Undo / Redo・MCP**:
  バックグラウンドのプラグイン状態保存は Undo に積まず Redo も消さない（[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
  CLI 書出しは `DocumentHost` の復旧スナップショットを使う。
  テストで既存パッケージの JSON だけを上書きしても保存済み履歴は置き換わらない。文書の編集経路を使うか、別の入力は新しいパッケージへ書く。
  MCP（`echode mcp SOCKET`）は接続先の `echode serve PACKAGE SOCKET` と同じビルド・絶対ソケットパス・API 版 1 を確認し、stdout へログを混ぜない（[ADR 0115](decisions/0115-phase2-mcp.md)）。

- **MCP が `request id must be a string or integer` を返す・通知の応答が来ない**:
  `mcp.rs::McpSession::reply` の ID 検証と通知の早期 return を確認する。
  MCP は null・真偽値・配列・オブジェクト・小数の ID を拒否する。null が正当な JSON-RPC API とは異なる。
  ID の省略は通知として応答せず、`tools/call` を装っても接続・編集しない。
  `mcp_requests.rs` と必須契約 `mcp-request-identity` は単一／一括編集・Undo / Redo の未送信と履歴保持を検査する。
  正当な文字列・整数 ID まで拒否される場合は、元の型と旧・新 MCP の初期化・メタデータを分けて照合する
  （[ADR 0115 の入力回帰](decisions/0115-phase2-mcp.md#要求-id-と通知の回帰補強2026-10-29)）。

- **旧 MCP が `initialize first` / `already initialized` / `request id was already used` を返す**:
  `initialize` の成功だけでは編集可能にならない。続く ID なしの `notifications/initialized` が必要である。
  初期化前の通知、null・配列等の params、ID を持つ同名の要求は準備完了へ遷移させない。
  使用済み ID は型も照合し、数値 `1` と文字列 `"1"` を混同しない。dispatch 済みの失敗にも新しい ID を使う。
  `mcp_lifecycle.rs` と必須契約 `mcp-legacy-lifecycle` は同じ stdio と実ソケットで、未送信・履歴・再オープンを検査する。
  ここでの拒否は接続先 API の版エラーや、編集後の結果不明とは異なる
  （[ADR 0115 の初期化回帰](decisions/0115-phase2-mcp.md#旧-mcp-の初期化と-id-再使用の回帰補強2026-10-29)）。

- **新 MCP が `unsupported protocol version` / `clientCapabilities must be an object` を返す**:
  接続先 API の版ではなく、各要求の `params._meta` の MCP 版と capabilities を確認する。
  版のキーを持つ要求は旧 MCP の準備完了に関係なく `validate_modern` を通り、未知・旧版の文字列は `-32022`、型の不正は `-32602` になる。
  直前の正当な新 MCP 要求からメタデータを引き継がず、初期化していない旧 MCP の入力も編集を解放しない。
  `mcp_metadata.rs` と必須契約 `mcp-modern-metadata` は接続前の拒否・履歴保持・後続の正当な編集と再オープンを検査する
  （[ADR 0115 のメタデータ回帰](decisions/0115-phase2-mcp.md#新-mcp-の要求ごとのメタデータの回帰補強2026-10-29)）。

- **MCP のツール引数が拒否される・拒否したのに履歴が変わる**:
  `mcp.rs::call_tool` はツール名からアクションの `type` を決める。呼出側の `type` は同じ値・null でも拒否する。
  Undo / Redo・概要は引数なし、一括編集は `actions` だけを受ける。省略した arguments と明示した null は区別する。
  型の不正・未知のツールは `-32602`、既知のツールの余分な引数は `isError` の実行エラーになり、どちらも接続しない。
  `mcp_arguments.rs` と必須契約 `mcp-local-arguments` は接続数・ジャーナル・既存 Redo・後続の編集と再オープンを検査する。
  接続先で拒否したアクションや結果不明とは段階が異なり、この拒否では自動再送や履歴の巻戻しは不要である
  （[ADR 0115 の引数回帰](decisions/0115-phase2-mcp.md#ツール引数のローカル拒否の回帰補強2026-10-29)）。

- **MCP が `unsupported Echode session protocol` を返す**:
  MCP 自体のプロトコル版ではなく、接続先 API の `protocol.version` が返す整数の版 1 を確認する。
  この段階での拒否では編集をまだ送っていない。編集後の応答破損や切断による結果不明と区別する。
  `mcp_version.rs` と必須契約 `mcp-backend-version` は、実ソケットで未送信・履歴保持・再オープンを確認する。
  正しい版 1 の追加フィールドは拒否しない（[ADR 0115 の版確認](decisions/0115-phase2-mcp.md#接続先の版確認の回帰補強2026-10-29)）。

- **MCP が `invalid session response` と結果不明を返す**:
  接続先の版・ID・result/error の排他と、error の整数 code・文字列 message を照合する。
  `error:null` やフィールドの欠落・型違いは通常の実行エラーではなく不正な応答である。
  編集が既に記録済みの可能性があるため、概要を確認せずに再送しない。原本の編集を自動で巻き戻さない。
  `mcp_responses.rs` と必須契約 `mcp-backend-errors` は、実際の編集後に応答だけを壊し、
  結果不明の案内・再送禁止・単一 Undo / Redo と再オープンを検査する
  （[ADR 0115 の応答検証](decisions/0115-phase2-mcp.md#接続先のエラー応答の形の検証2026-10-29)）。
  応答の ID は要求した整数と同一でなければならず、文字列・小数へ読み替えない。
  外枠は `mcp-backend-envelope`、版応答の外枠は `mcp-version-envelope` で別々に検査する。
  版確認中の外枠拒否なら編集は未送信だが、編集後の拒否なら既に確定している可能性がある。
  同じ保守的な結果不明の案内でも、どの段階の応答かを分けて調べる
  （[ADR 0115 の外枠回帰](decisions/0115-phase2-mcp.md#接続先の応答外枠の回帰補強2026-10-29)）。
  完全な応答を送った後、検査用サーバーの `shutdown(Write)` だけが `NotConnected` になる場合は、先に正常に閉じた相手との競合を調べる。
  `mcp_responses.rs` は半切断せず後続の読込みで EOF と再送禁止を検査する。部分行の拒否に必要な半切断とは区別する。

- **MCP 改行回帰で `backend EOF wait` が 5 秒の期限切れになる**:
  2026-10-30 の検査基盤改定で、`mcp_framing` の版確認中・部分応答後の切断待ちが全体・単独とも一度ずつ失敗した。
  後の同じ構成の単独実行では再現しなかった。子プロセスの起動前ではなく、部分応答後の EOF 待ちという別の境界である。
  `handshake`・`suffix`・`following` と両側のスレッドを調べ、走査ヘルパーの起動遅延と同じ原因だと断定しない。
  期限・期待値・検査対象は維持し、失敗と再実行を時間履歴・個別ログで区別する。根因は未確定である。
- **MCP が `MCP message ended without a newline` を返す**:
  stdio 入力と接続先 API の応答は同じ `mcp.rs::read_line` を通るため、どちらの EOF かを切り分ける。
  stdio の部分行は編集前に拒否し、完了済みの前方の要求だけを保持する。CR だけの終端では完了しない。
  版応答の部分行では編集を送らないが、編集応答の部分行では編集が既に成立している可能性がある。
  `isError` と結果不明の案内を確認し、概要を調べずに再送しない。
  `mcp_framing.rs` と必須契約 `mcp-stdio-framing`・`mcp-backend-framing` が両境界を実ソケットで区別する
  （[ADR 0115 の回帰補強](decisions/0115-phase2-mcp.md#改行境界の回帰補強2026-10-29)）。
  2026-10-29 のエラー応答検証では、通常の `mcp-backend-framing` が部分応答後の EOF 待ちで 2 回期限切れとなった。
  `read_to_string` が 5 秒後に `WouldBlock`（Darwin errno 35）を返したもので、応答形のアサーション失敗ではない。
  追加した診断では初回の版応答（`handshake=true, suffix=""`）後で、受信済みの `following` は空だった。
  単独実行と正式な再実行では再現しない場合もあり、原因は未確定である。期限や対象は変更しない。
  次回も診断の `handshake`・`suffix`・受信済みの `following` で版応答と編集応答を切り分ける。
  単独の記録は `target/focused-checks/mcp-framing-timeout/`、正式な再実行は検査レポートで確認する。

### サニタイザー（TSan / ASan / RTSan）

- **TSan の既知の環境要因と製品不具合の切り分け**:
  1. UI テストは機器なしの応答差し替え口を使い、実 HAL を開かない（`target/macos27-audit/tsan-hal-before.log`）。
     `EchodeAppTests` のバンドルでは `RealHALGuard.swift` が HAL の関数をリンク時に置き換え、機器を開かずに失敗させる（[ADR 0209](decisions/0209-real-hal-guard.md)）。
     `Real HAL reached:` で全体実行が失敗し、Swift Testing がテストを「unknown」と示したら、`make check-real-hal` がテスト名を示す。
     そのテストに `sendDevice`、`sendAudio`、`sendAudition` のいずれかを注入する。
     名前のテストに心当たりがなければ、併記された直前のテストが待たずに残した要求を調べる。
     単体候補の追加は `opensWindow` からデバイス要求も送るため、再生なし・`preparesOutput = false` でも実 HAL へ到達し得る。
     `PluginBatchTests.invalidTargetsDoNotPartiallyInsertOrClearRedo` は `FakeParameterPlugin` を注入し、窓要求を待って対象も照合する。
     実 HAL の `write_planes` と別スレッドの過去の書込みが競合した報告は、`EchodeAppTests` からは出なくなった。
     出たら `EchodeBridgeTests` か `spike-*` など、実 HAL を使う経路を調べる。警告自体は抑制しない。
  2. 終了済みと新規のスレッドの `errno` 誤検出は、C のみでも再現し、検査ランタイムの寿命処理を修正した（[ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)）。
     `target/bridge/thread/errno/` の baseline・fixed・real-race の対照と、同じランタイムのロード証拠を確認する。
     準備が古い時は `make setup-bridge` を使う。取得済みソースの再ビルドには `python3 scripts/tsan_runtime.py --prepare` も使える。
  3. 継続が `await` より先に再開されると、再開側の書込みと `await` の直後の読取りを、TSan は順序なしと見る（[ADR 0207](decisions/0207-continuation-handoff-tsan.md)）。
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

## 再現と引き継ぎに残す情報

### 停止クリック・発音アタックの実出力診断

- `ECHODE_STOP_TRACE_DIR` を指定した診断実行では、停止から 50 ms 後までの HAL 実出力を `.f32le` と `.json` に記録する（通常ビルドは記録無効、[ADR 0104](decisions/0104-stop-output-diagnostics.md)）。
  `stop_frame - first_frame`、位相の連続性、`stop_fade_offset`、`cleanup_pending`（ライブ後片付けの非待機保留、[ADR 0105](decisions/0105-nonblocking-stop-cleanup.md)）、`render_ns` / `elapsed_ns` と `output_host_time` の呼出間隔を照合する。
  ユーザーの再生音が含まれるため、通常の `debug-report` には自動収集しない。
- 無音からの発音アタックは、別プロセスでの直接出力と製品の再生・出力補正後の全サンプルを比較し、音源本来のアタックとホストの追加変化を区別する（[ADR 0106](decisions/0106-natural-plugin-onsets.md)）。

### 共通の記録項目

1. 対象のコミットと作業ツリー、診断資料の場所、実行ファイルの出所。
2. 最小の入力と初期状態、操作順、期待と実際（文書・トラック・ノート等の ID を含む）。
3. 実行した正式な検査名、終了コード、期待値が実際に走った証拠、ログの場所。
4. 原因の根拠と未確定の部分（製品不良・テスト起動・ツール取得・環境条件の区別）。
5. 修正した境界と回帰テスト、未実行の検査・理由、残る制約。

`/tmp` だけの再現や画像を唯一の修正根拠にせず、通常のテストか登録プローブへ移して監査に参照を残す。
監査・ADR に引用する証拠は [証拠の保全](evidence/README.md) に従い、追跡する要約と再生成手順を残す。
過去の `target/` が欠けている時は、当時のローカル記録の消失として扱い、現在の検査ログで埋めない。
