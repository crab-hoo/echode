# 検査の手順

選択・依存関係・実行予算・必須契約の正本は [scripts/verification.json](../scripts/verification.json)。
実行器は `scripts/verify.py`。AGENTS、終了フック、CI に別の選択条件を持たせない。

## 作業の入口と完了

着手時に `make verify-plan` を実行する。
作業中は関係するテストを `-q` 付きで先に実行し（例: `cargo test -q -p echode-engine`）、最後の編集後に `make verify` を実行する。
HEAD と作業ツリーの差分には、ステージ済み・未ステージ・未追跡・削除・移動前後を含む。
コミット済みの変更には `VERIFY_BASE=<変更前のref> make verify` を使う（存在しない ref は失敗する）。

自動検査は選択されたものをすべて通す。
自動検査が失敗した時は、`scripts/verify.py` が該当ログから失敗箇所の抜粋を標準エラーへ表示する。
選択された計測・実機・ネットワーク検査は、明示して実行するか、未実行の理由を記録する。
理由なしの未実行が残れば `make verify` は非ゼロで終わる。終了フックが自動でウィンドウや音声デバイスを開くことはない。

```sh
# 手動検査の実行（他の自動検査も必要に応じて実行する）
make verify VERIFY_ARGS='--run spike-audio --run spike-playhead'
# 実機確認を依頼された場合は10秒の確認から選ぶ。長時間計測は必要な時だけ明示する
make verify VERIFY_ARGS='--run spike-engine-smoke'
# 選ばれた実機検査を見送る時は、その検査の入力に理由を結び付ける
make verify VERIFY_ARGS='--defer spike-audio --reason "今回の変更は文書だけで製品の音声経路に変更なし"'
make verify
```

`--all` は追加の長時間検査も含めて全検査を選択する。
10分計測は `make verify VERIFY_ARGS='--run spike-engine'` でも追加できる。
スケジューラー・リアルタイム処理の大きな変更、長時間でだけ起きる不具合、リリース前など、目的がある時に実行する。

`--only ID` は作業中の個別検査か CI の分割用で、完了報告前には使わない。
無関係な検査を追加実行する時は `python3 scripts/verify.py --all --only ID`、手動検査ならさらに `--run ID` を付ける。
自動検査の失敗は見送れない。手動検査の見送りも合格ではなく、最終報告に名前と理由を残す。

未コミットの変更が複数の作業にまたがっていても、各検査の依存入力が前回と同じなら結果を再利用する。
候補を一律に `--run` せず、今回の影響・過去の有効な結果・検査の目的を確認する。

## 自動検査

通常の `make check` は `check-verification`、`check-base`、`check-product-contracts` を順に 1 回ずつ実行する。
`scripts/verify.py` では JSON の `make_target` により、結果 ID `check` が `check-base` を実行し、基盤と製品契約は別の結果として記録する。

- `check-verification`: 検査選択、依存入力の失効、診断、署名、作業資料のリンクと行長整合、ADR・監査・スパイクを含む全文書のローカルリンク（`target/` の手元の証拠は除く）、macOS 27 / M4 のビルド基準と生成物の検査（[ADR 0114](decisions/0114-m4-build-baseline.md)）。文書だけの変更ではこれを再実行し、入力が変わっていない製品検査は再利用する。
- `check` (`check-base`): Rust / Swift のビルドと全テスト、ランダムな編集の不変条件・Undo / Redo・保存往復、基準音声、保存形式、ブロック長比較、プラグインのクラッシュ・ハング、clippy、FFI、ライセンス。
- `check-product-contracts`: [scripts/verification.json](../scripts/verification.json) の `product_contracts` に定義された全必須契約テストを実行する。
  指定した実テストが 1 件走った証拠を要求し、削除・無視・0 件を拒否する。
  さらに一時コピー上で各契約の `negative_control`（既知の不具合を戻す差分）を適用し、テストの期待値で失敗しなければ検査全体を失敗とする（コンパイル失敗は検出の証拠にしない）。
  各契約の ID・対象テスト・目的・負の対照は `scripts/verification.json` を正本とし、本書へ重複記載しない（[ADR 0093](decisions/0093-product-contract-gate.md)、[ADR 0119](decisions/0119-agent-token-efficiency.md)）。
- `rtsan`: テスト用シンセ、計画の受け渡し、製品の再生・トランスポート・出力バッファ、プラグインヘルパーの処理区間でリアルタイム処理の禁止操作（ロック、確保、ブロックするシステムコール）を検出する。
- `check-input`: Swift の入力の単体テスト。ウィンドウは開かない。
- `check-bridge-features`: 橋渡しの全 feature、Rust テスト・clippy・生成器を含むライセンス。
- `check-tsan-runtime`: C だけのスレッド再利用、寿命修正を外した負の対照、生存中の errno を共有する実競合の検出を確認する。
- `check-bridge-asan`: 製品 ABI・音声出力バッファの Rust 検査と、Swift の編集・保存・再読込を別プロセスでメモリ検査する。
- `check-bridge-tsan`: 標準ライブラリを含む製品の Rust と Swift を、同じ固定ランタイムで競合検査する。
  読み込まれたランタイムも照合する（[ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)）。
- `check-bridge-miri`: Rust 側のポインタ・所有権。製品のスナップショット保持、UTF-8 の借用、可視列の容量・null を seed 1・17・42 で検査する。

## 実機・計測・ネットワークの検査

実機・聴感の判定は原則としてユーザーが担当する。
実機を使う検査は明示的な依頼がある時だけ `--run ID` で実行し、ユーザーが担当する項目は理由付きで `--defer ID --reason 理由` で見送る。
ウィンドウを使う検査は現在の Space で表示条件を満たせる環境で実行する。

- `spike-isolation`: 隔離の release 計測と製品 CLAP の実時間ブロック期限。明示的な依頼がある時に実行し、[S3](spikes/S3-plugin-isolation.md) の数値を更新する。
- `spike-draw`: Metal の大規模シーンと可視判定の実機計測（[S1](spikes/S1-drawing.md)）。
- `spike-arrange-frame`: 画面を開かず製品の Metal 描画を読み戻し、倍率 1・2、ズーム・スクロール、幅、文字、行境界とヘッダへのはみ出しを比較する（`target/spike-arrange-frame.json`）。
- `spike-piano-frame`: 画面を開かず製品の Metal 描画を読み戻し、鍵盤・ノートの音高行、調に依存しない白黒、ディグリー表示を比較する（`target/spike-piano-frame.json`）。
- `spike-app`: 製品のアレンジとピアノロールを実表示し、メインスレッドの 0.5 秒の負荷中も描画することとアクセシビリティ ID を確認する（`target/spike-app.json`）。
- `spike-playhead`: 専用 AppKit プローブ（`EchodePlayheadProbe`）で実ウィンドウのプレイヘッド移動・再生・一時停止・休止・破棄・文書交換を確認する（`target/spike-playhead.json`）。
- `spike-history`: 大きい文書の保存・復元と、無関係なノートを増やしたゲイン差分の release 計測（[S4](spikes/S4-history.md)）。
- `spike-input`: 確認ウィンドウでピンチ・触覚・感圧を実機確認する（[S5](spikes/S5-input.md)）。
- `spike-engine-smoke`: 現在の音声機器のレートで 1,000 ノードの処理を 10 秒確認する（`target/spike-s2-smoke.txt`）。
- `spike-engine`: 明示実行用の 10 分計測（600 秒）と逆順イベント整列の release 計測（[S2](spikes/S2-engine.md)、[ADR 0096](decisions/0096-measurement-completion.md)）。途中停止・機器変更・中断は不合格とする。
- `spike-audio`: 製品の CoreAudio 出力を無音で開き、コールバック、停止・再起動、機器一覧と通知登録、再生中の計画更新・位置移動を確認する。
- `spike-bridge`: スナップショットと配列の受け渡しの release 計測。
- `spike-render`: WAV の一括書き出しの release 計測。
- `advisories`: ネットワークで依存ライブラリの脆弱性情報を確認する。

`make known-bugs` は登録済みの未解決の再現（`audit_*`）を実行する入口であり、`make bless` は意図した音の変更だけに使う。両方とも自動検査には含めない。

## 記録と有効期限

結果は `target/verification/report.json`、`report.md`、各 `<ID>.log` に記録する（製品契約の詳細は `product-contracts.json` と `product-contracts/`）。

- `passed`: その検査の依存入力とツールで合格。
- `failed`: ログと標準エラーの抜粋を調査して修正し、再実行する。
- `pending` / `running`: 自動検査の未完了。終了報告には使えない。
- `stale`: 検査中の変更や依存入力の更新で無効。再実行する。
- `not_run`: 手動検査の未実行。実行か理由の記録が必要。
- `deferred`: 理由付きの未実行。合格ではない。

検査ごとの `inputs` と `packages`（Cargo のローカル path 依存を再帰的に含む）、共通設定、ツールの版と解決パスをハッシュ化して識別する。
`tool_inputs` に指定した検査専用ランタイムは、Git の対象外でも実バイトをハッシュ化する。
文書だけの変更は製品コードの検査結果を失効させない。
`device_context` を持つ実機計測は、`system_profiler` で読み取った既定出力デバイスのメタデータのハッシュも識別に含める。
`target/verification/.lock` の排他ロックを保持する間はファイル変更を待つ。`make doctor` はロックの保持状態と各検査結果の鮮度（`current` / `partial` / `stale`）を確認する。

## 環境、フックと CI

新しい環境では `make setup`、動的検査には `make setup-bridge` を実行する。
製品のコンパイラーは `rust-toolchain.toml`、橋渡しの動的検査（ASan / TSan / Miri）は `scripts/bridge-toolchain.env` の固定 nightly を使う（[ADR 0044](decisions/0044-swift-bridge.md)）。
TSan のセットアップには CMake と Ninja も必要で、固定した公式 LLVM のソースを取得する。
検査専用ランタイムの準備・再生成は `make setup-bridge`、取得済みソースだけの再ビルドは `python3 scripts/tsan_runtime.py --prepare` を使う。
Xcode や製品アプリは変更しない。通常検査はネットワークを使わず、古い準備は失敗として報告する。

`.cursor/hooks.json` の `preToolUse` は編集ツールをロック解除まで待たせ、`stop` は `completed` イベントで `scripts/verify.py` を呼ぶ。
`.codex/hooks.json` もアダプターを備えるが、設定ファイルの存在だけでホスト側のフック発火済みとは扱わない。フックのない環境でも `make verify` は必須である。
CI の自動検査一覧は `scripts/verification.json` から生成し、各ジョブが `--all --only ID` で分担する。調査資料の収集は [デバッグ手順](debugging.md) を参照する。
