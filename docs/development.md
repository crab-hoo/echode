# エージェントの開発手順

必須規則は [AGENTS.md](../AGENTS.md)。この文書は実際の作業の案内である。

検査基盤は Python 3.11 以降を使う。Rust のローカル依存を読むために標準の TOML パーサーを使用し、追加の Python パッケージは必要ない。

## 開始から完了まで

1. `git status --short` で既存変更を把握する。他の作業の変更を巻き戻さない。`make doctor` で不足ツールとアプリ・検査の古さ、`make verify-plan` で対象を確認する。
2. [現在の制約](status.md)、PLAN・ARCHITECTURE の該当節、ADR、[監査の読み順](README.md#監査の読み順) から触る領域を読む。設計上ある機能と接続済みの機能を区別し、下記の保守の観点から今回の影響範囲も確認する。
3. 不具合は最小入力、初期状態、操作順、期待と実際、使用した版を記録する。製品の入口から再現し、回帰テストを先に失敗させる。環境エラーは [デバッグ手順](debugging.md) で分ける。
4. 境界と不変条件を守って修正し、必要なドキュメント・テスト・デバッグ基盤も同じ作業で更新する。途中で検査漏れや新しい失敗条件が判明したら、保守の対象を見直す。新しい判断は ADR、発見・修正の証拠は監査へ残す。
5. 完了前に実装と基盤の整合を確認し、必要な更新を終えてから関係するテストと `make verify`。選択された実機・計測は実行するか理由を記録し、最後の編集後に全選択項目を確認する。途中の `--only` の成功を完了の証拠にしない。
6. 最終報告には変更の効果、基盤の更新内容（不要なら理由）、検査結果、見送った検査名と理由、残る制約を簡潔に記す。コミット・プッシュはユーザーの依頼がある時だけ行う。

実機・聴感テストは原則としてユーザーが担当する。エージェントは実機を使わない回帰を先に行う。音声だけの変更に Computer Use を使わず、機器を開く検査やアプリ操作は明示的な依頼がある時だけ行う。

検査候補が多いことを理由に全実機検査を一律に動かさない。前の作業の変更が残っている時も、各検査の依存入力が同じなら合格を再利用する。通常の音声変更は波形と製品バッファで確認し、ユーザーが実機検査を依頼した場合は短時間確認から選ぶ。10分計測は大きなリアルタイム設計変更・長時間不具合・リリース確認など、目的を説明できる時に追加する。回帰検査で操作の順序と実際の出力を確認することを、同じ処理の反復回数で置き換えない。

## 基盤の保守を同じ作業で行う

開始時に今回の変更と下記の関係を確認し、作業中に分かった事実と完了時の実装に合わせて見直す。必要性に応じて更新し、変更がない資料や基盤を形式的に書き換えない。実装だけ終え、必要な保守を別の依頼へ残した状態では完了にしない。

### ドキュメント

利用方法・コマンド・設定が変われば README と該当する手順、実装範囲・制約が変われば status、設計や判断が変われば ARCHITECTURE と ADR、方針・完了条件が変われば PLAN を更新する。モジュールの移動や新しい資料には所在と索引も追従させる。どの資料が正本かは [資料の入口](README.md) に従い、同じ事実をすべての資料へ複製しない。

監査の指摘を直したら、その項目に対応と回帰テストを追記する。過去の失敗・測定・未実施は当時の証拠として保持し、現在の状態と区別する。機能が接続された時は、関連する未実装・未検証の記述を根拠とともに見直す。

### テスト基盤

振る舞いや境界を追加・変更したら、通常・異常・境界の入力を既存のテストが通せるか確認する。新しい不具合は再現を回帰テストへ移す。新しいアクションや音を作る処理には、この文書の回帰テストの基準を適用する。

モジュール・依存・ツールチェーン・実行入口が変わったら、`scripts/verification.json` の選択と入力の識別、Makefile、CI、検査環境の準備、実行手順を照合する。対応条件は JSON だけに置く。検査を追加・変更したら、選択のテストと実際の実行結果も確認する。

一時リポジトリなどで検査基盤自身をテストする場合は、親プロジェクトの `VERIFY_BASE` のように対象へ属する環境設定を fixture から隔離する。通常の入口に加え、基準 ref を指定するコミット済み変更の入口でもテストが動くことを確認する。

必須契約のテスト名・製品の経路・誤りを戻す差分が有効か確認する。実装の変更で差分の適用先が変わった場合も、同じ不具合を再現する差分へ更新し、期待値で拒否される証拠を残す。基準音声・期待値・時間の閾値を変更する時は、仕様の変更と検査の不具合を区別し、失敗を消す目的で緩めない。

### デバッグ基盤

新しい失敗経路・環境依存・ツールの取得エラーが判明したら、[デバッグ手順](debugging.md) に識別方法と再現・調査先を反映する。繰り返す問題を手順だけで解けない場合は、診断コマンド、状態・エラー、検査の起動方法も必要な範囲で改善する。一時的な調査で終えず、次のエージェントが同じ原因を判別できる証拠を残す。

ログ・成果物の出力先や形式、新しいヘルパー・バンドル・外部入力を変えたら、doctor の鮮度・出所の照合と debug-report の収集範囲を見直す。診断の出力形式を変えれば、利用する手順とテストも更新する。古い結果・環境不足・製品不良を区別できること、秘密情報やユーザー文書を収集しないこと、音声スレッドへ禁止操作を持ち込まないことを維持する。

### 完了時の確認

実装、説明、検査が保証する範囲、失敗時に集められる証拠が一致することを確認する。新しい制約や保証できない範囲は正本へ記録する。最終報告では今回の基盤の更新を短く示す。例えば「操作変更に合わせて利用手順と入力回帰を更新、診断の収集範囲は変更不要」のように、必要な判断が分かればよい。

## コードの所在

- 文書モデルは `crates/echode-model`、変更・Undo / Redo は `crates/echode-actions`。保存、ジャーナル、復旧、内容アドレスの保管庫は `crates/echode-history`。
- 音声計画、再生位置、MIDI、内蔵音源、サンプルレート変換、ループ、フェーダー、CLAP の信号経路は `crates/echode-engine`。製品の HAL 出力・通知と機器追従は `crates/echode-audio-io`。共通の RT 検出は `crates/echode-realtime`。
- 素材の平面とSRCは `echode-audio-io/src/clips.rs`、配置とループは `echode-engine/src/playback.rs`、バスの受渡しは `plugins.rs`、ゲインの状態と f64 加算は `mix.rs`。内容位置 0 は配置原点からのテンポ積分で、波形の `source_span` も同じ区間を使う。CLAP のない書き出しも `render.rs` から同じ再生器を使う（[ADR 0095](decisions/0095-product-audio-buses-and-continuity.md)、[ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)）。
- ループ先頭を含む準備済みPCMの保持・失効は`clips.rs`の`media_loader`。フェーダー前の出力元の差し替えは`playback.rs`の`CachedTrack` / `set_cached_tracks`。供給側のキャッシュ生成はPhase 3。P1の4条件を同じ文書で通す統合契約は`echode-cli/tests/render.rs`に置く（[ADR 0108](decisions/0108-phase1-loop-readiness.md)、[最終照合](reviews/2026-10-17-phase1-acceptance.md)）。
- ライブ出力の段差補正は `echode-engine/src/declick.rs` を HAL から使用する。定期保存の制御ハンドルは `echode-sandbox/src/session.rs` の `PluginControl`。音声グラフは `PlaybackAudio` だけが持ち、制御側のカタログから用意した計画と音声バッファを一緒に交換する。共有メモリの準備と採用も分ける（[ADR 0101](decisions/0101-live-output-discontinuities.md)、[ADR 0102](decisions/0102-exclusive-audio-graph.md)）。
- 任意の停止出力診断は `echode-audio-io/src/trace.rs`。HALの実バッファを事前確保領域へ記録し、制御側で保存する。再描画した音を実出力の証拠へ代用しない（[ADR 0104](decisions/0104-stop-output-diagnostics.md)）。
- ライブ停止後の破棄する処理は `echode-engine/src/plugins.rs` の各ステージが保留し、完了をポーリングする。音声の出力を待つ通常の集約と混同しない。再開と計画交換は `playback.rs` が完了まで保留する（[ADR 0105](decisions/0105-nonblocking-stop-cleanup.md)）。
- 共有メモリは `crates/echode-sandbox`、本体外のロード・スキャン・処理は `crates/echode-plugin-host`、故障注入用 CLAP は `crates/echode-test-plugin`。致命信号の印は `echode-sandbox/src/region.rs` の `arm_abort_notice`。Workgroup の起動時ポーリングは `workgroup.rs`。スキャンは `echode-plugin-host/src/scan.rs`（[ADR 0111](decisions/0111-macos-helper-notices.md)）。
- CLI の render / serve は `crates/echode-cli`、ローカル JSON-RPC は `crates/echode-api`。製品 C ABI は `crates/echode-ffi`、Swift の保持と非同期処理は `swift/EchodeBridge`。
- Phase 2 の MCP stdio は `echode-api/src/mcp.rs`、CLI は `echode mcp SOCKET`。同じビルドのヘッドレスセッションへ接続し、アクションの登録簿・既存の API と書き手を使う。実プロセスの統合回帰は `echode-cli/tests/mcp.rs`。GUI への接続や再生操作を実装済みと扱わない（[ADR 0115](decisions/0115-phase2-mcp.md)）。
- 取消世代は `echode-audio-io/src/transport.rs`、投入順序は `echode-ffi/src/queue.rs`。準備の完了応答と、停止による発音禁止を別々に確認する（[ADR 0097](decisions/0097-cancellable-retained-transport.md)）。
- 製品 UI は `swift/EchodeApp/Sources/EchodeAppCore`。描画・ヘッダ入力・ワークスペース・controller の責務を分ける。実ウィンドウ検査は `EchodePlayheadProbe` と `Tests/EchodePlayheadChecks`、通常のテストは `Tests/EchodeAppTests`。
- ミックス操作の配置と下書きは `MixControls.swift`、数値入力とピーク表示は `MixInspectorView.swift`、ドラッグの試聴・確定は `ProjectController.swift`。不変のパラメーター・クリップ値は `echode-ffi/src/controls.rs` と `EchodeBridge/Controls.swift`、音声所有のピークは `echode-engine/src/meter.rs`、一時フェーダーと計画採用は `playback.rs`。仕様と検査は [ADR 0107](decisions/0107-mix-controls.md)。
- 表示リンクの範囲は `CanvasRenderer.swift` の `displayFrameRateRange`、画面の最大は `ArrangeView` と `PianoRollView` が渡す。波形の再利用バッファは `ArrangePainter.swift`。ピークのキャッシュヒットと部分読出は `echode-history/src/peaks.rs`。再生中の稼働表明は `echode-audio-io/src/activity.rs`、CLI の書き出しは `echode-cli`、処理中のヘルパーは `echode-plugin-host/src/awake.rs`（[ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)）。
- 描画の大規模な検証は `crates/echode-draw` と `spikes/draw`。入力の前提検証は `spikes/input`。`spike` の型を製品モデルへ流用しない。
- 検査の選択・予算・必須契約は `scripts/verification.json`、実行器は `scripts/verify.py`、検査基盤自体のテストは `scripts/tests`。フックは薄いアダプターに留める。

## 回帰テストを選ぶ基準

内部関数の戻り値だけでなく、失敗した境界を通す。再生位置なら HAL 登録前の製品の準備経路、プラグインなら実形式のバンドルを別プロセスでスキャンして処理、キー入力なら `NSView.keyDown` から上位への転送を確認する。

- 音を変える処理には `crates/echode-engine/tests/block_size.rs` の比較を追加する。位置・チャンネル・長さなどの境界値も使う。
- アクションを足す時は `crates/echode-history/tests/properties.rs` の生成へ追加し、不変条件、Undo / Redo、保存・読込を通す。
- 保存・復旧は成功応答の後に再オープンする。I/O 失敗、途切れた記録、二重書き手、リンクを含む入力も関係範囲で扱う。
- Swift は実 View の入力、controller の操作、モデルへの反映を混同しない。描画器を計測モードで強制起動した成功を通常の休止→再開の成功へ読み替えない。
- 固定のテスト入力を特別扱いする製品分岐を作らない。スキップ、0件、期待値へ合わせた基準の再生成で失敗を消さない。

必須契約を追加する時は既存の `product_contracts` を参考に、`scripts/verification.json` へ名前・実行経路・目的・誤りを戻す差分を登録する。正しいソースで実テストが実行され、誤りを戻したコピーではその期待値で失敗することを確認する。コンパイル失敗は検出の証拠にならない。新しい独立の検査は Makefile と同じ JSON へ登録し、選択のテストも更新する。

資料の役割、コマンド、ローカルリンクの破損も通常検査で確認する。機能の追加は、その領域のテストを足してから資料の現在の範囲を更新する。

### macOS の Workgroup と再利用の境界

選択デバイスの Workgroup の取得と周期は `echode-audio-io/src/workgroup.rs`、変更通知は `hardware.rs`、出力の再準備は `stream.rs` / `session.rs`。保持とヘルパーへの引継ぎは `echode-sandbox/src/workgroup.rs` / `session.rs` と製品の `echode-engine/src/plugins.rs` にある。素材・配置のキャッシュは `echode-audio-io/src/clips.rs`、描画の可視列は `CanvasGeometryCache.swift`。条件・回帰・残る範囲は [ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md) を参照する。

### ビルド対象の CPU 基準

Rust と C / C++ 依存は `.cargo/config.toml`、Swift Package は各 `Package.swift`、Xcode の入口は `project.yml` を使う。Swift は CPU 指定と Clang import の指定を対にする。直接コンパイルする補助コードは `scripts/build-platform.env` に揃える。macOS 27 / M4 以降の実行機種の判定は `scripts/check_build_host.py`。変更時は新しいキャッシュでの import と実際の IR、検査の失効も確認する（[ADR 0114](decisions/0114-m4-build-baseline.md)）。
