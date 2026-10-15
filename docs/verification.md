# 検査の手順

選択・依存関係・実行予算・必須契約の正本は [scripts/verification.json](../scripts/verification.json)。実行器は `scripts/verify.py`。AGENTS、終了フック、CI に別の選択条件を持たせない。

## 作業の入口と完了

着手時に `make verify-plan`。作業中は関係するテストを先に実行し、最後の編集後に `make verify`。HEAD と作業ツリーの差分には、ステージ済み・未ステージ・未追跡・削除・移動前後を含む。コミット済みの変更には `VERIFY_BASE=<変更前のref> make verify` を使う。存在しない ref は失敗する。

自動検査は選択されたものをすべて通す。選択された計測・実機・ネットワーク検査は、明示して実行するか、未実行の理由を記録する。理由なしの未実行が残れば `make verify` は非ゼロで終わる。終了フックが自動でウィンドウや音声デバイスを開くことはない。

```sh
# 手動検査の実行（他の自動検査も必要に応じて実行する）
make verify VERIFY_ARGS='--run spike-audio --run spike-playhead'
# 未実行の理由はこのソースに結び付ける
make verify VERIFY_ARGS='--defer spike-engine --reason "10分間の実機負荷測定は今回未実施"'
make verify
```

`--all` は全検査を選択する。`--only ID` は作業中の個別検査か CI の分割用で、完了報告前には使わない。無関係な検査を追加実行する時は `python3 scripts/verify.py --all --only ID`、手動検査ならさらに `--run ID`。自動検査の失敗は見送れない。手動検査の見送りも合格ではなく、最終報告に名前と理由を残す。

通常検査は全変更で選択する。共通依存・ツールチェーン・検査基盤・フック・CI の変更では全自動検査を選ぶ。手動検査は関係する領域の変更から選ぶため、Cargo.lock だけで全実機検査を要求しない。パスでは意味の影響をすべて判定できないので、必要な検査は追加する。

## 自動検査

- `check`: Rust / Swift、ランダムな編集の不変条件・Undo / Redo・保存の往復、音声の基準比較、保存形式、ブロック長比較、プラグインのクラッシュ・ハング、lint、FFI、ライセンス。検査基盤自体は `check-verification` で確認する。
- `check-product-contracts`: 実 macOS CLAP バンドルのスキャン→キャッシュ→楽器・エフェクト処理と失敗、初回音声処理前の位置、ネイティブ View のキー転送。指定した実テストが1件走った証拠を要求し、削除・無視・0件を拒否する。既知の誤りを一時コピーへ戻し、期待値で失敗しなければ検査も失敗する。コンパイル失敗は検出の証拠にしない（[ADR 0093](decisions/0093-product-contract-gate.md)）。
- `rtsan`: テスト用シンセ、計画の受け渡し、製品の再生・トランスポート・出力バッファ、プラグインヘルパーの処理区間の禁止操作。HAL 全体や未実装音源を網羅する検査ではない。
- `check-input`: Swift の入力の単体テスト。ウィンドウは開かない。
- `check-bridge-features`: 橋渡しの全 feature、Rust テスト・clippy・生成器を含むライセンス。
- `check-bridge-asan`: 製品 ABI・音声出力バッファの Rust 検査と、Swift の編集・保存・再読込を別プロセスでメモリ検査する。
- `check-bridge-tsan`: 標準ライブラリを含む製品の Rust 静的ライブラリと Swift を同じプロセスで競合検査する。
- `check-bridge-miri`: Rust 側のポインタ・所有権。製品のスナップショット保持、UTF-8 の借用、可視列の容量・null を seed 1・17・42 で検査する。パッケージ I/O や Swift、Metal は対象外。

通常の `make check` は `check-base` と `check-product-contracts` を順に1回ずつ実行する。実行器では JSON の `make_target` により、結果 ID `check` が `check-base` を実行し、製品契約は別の結果として記録する。CI も同じ分割を使い、同じ契約を二重に実行しない。

Swift の通常検査には、配置と保存、選択と入力の下書き、コマンドの有効条件、ファイル一覧の非同期更新、View 再作成時の表示位置、描画結果の世代・版も含む。実際のマウス・キー操作を網羅した主張とは区別する。

## 実機・計測・ネットワークの検査

実行は `--run ID`。ウィンドウを使う検査は現在の Space で表示条件を満たせる環境で実行する。期限は検査の予算であり、製品の性能の閾値とは別である。

- `spike-isolation`: 隔離の release 計測と製品 CLAP の実時間ブロック期限。隔離を変えたら実行し、[S3](spikes/S3-plugin-isolation.md) の数値を更新する。通常の音声比較には性能閾値を持ち込まない。
- `spike-draw`: Metal の大規模シーンと可視判定の実機計測。
- `spike-arrange-frame`: 画面を開かず製品の Metal 描画を読み戻す。倍率1・2、部分行、ズーム・スクロール、幅、選択、文字、ヘッダへのはみ出し。`target/spike-arrange-frame.json` と PNG。
- `spike-piano-frame`: 同じく画面なし。鍵盤・ノートの音高行、調に依存しない白黒、ディグリーの文字・内外の色・区切り。`target/spike-piano-frame.json` と PNG。
- `spike-app`: 製品のアレンジとピアノロールを実表示し、メインスレッドの0.5秒の負荷中も描画すること、共有する左右の行とアクセシビリティの ID を確認する。小規模な接続確認で、大規模性能の代用ではない。`target/spike-app.json`。
- `spike-playhead`: 専用 AppKit プローブの起動完了後、イベントループで実行する。計測モードで強制起動せず、0→2拍、再生、一時停止、休止、破棄、計画・文書交換を確認する。現在の Space・非遮蔽を満たさなければ未検証項目を保存して失敗する。状態は close の前に記録する。`target/spike-playhead.json`。通常の `swift test` からは分離している。
- `spike-history`: 大きい文書の保存・復元と、無関係なノートを増やしたゲイン差分の release 計測。
- `spike-input`: 実際にピンチ・触覚・感圧を確認する。ウィンドウを閉じるだけでは確認にならない。
- `spike-engine`: 音声デバイスを使う10分間の計測と、逆順イベント整列の release 計測。
- `spike-audio`: 製品の CoreAudio 出力を無音で開き、コールバック、停止・再起動、機器の一覧と通知登録、再生中の計画更新・位置移動。画面は開かない。抜き差しや聴感の代用ではない。
- `spike-bridge`: スナップショットと配列の受け渡しの計測。
- `spike-render`: WAV の一括書き出しの release 計測。正しさのテストに時間の閾値を入れない。
- `advisories`: ネットワークで依存の脆弱性情報を確認する。

`make known-bugs` は登録済みの未解決の再現を実行する入口で、未登録の指摘もある。修正後は再現テストの ignore を外し、監査に対応を記録する。`make bless` は意図した音の変更だけに使う。両方とも自動検査へ入れない。

## 記録と有効期限

`target/verification/report.json` と `report.md`、各 `<ID>.log` に記録する。製品契約の詳細は `product-contracts.json` と `product-contracts/`。状態は次の通り。

- `passed`: そのソースとツールで合格。
- `failed`: ログを調査して修正し、再実行する。
- `pending` / `running`: 自動検査の未完了。終了報告には使えない。
- `stale`: 検査中の変更などで無効。再実行する。
- `not_run`: 手動検査の未実行。実行か理由の記録が必要。
- `deferred`: 理由付きの未実行。合格ではない。

全 tracked・非 ignored untracked ファイルの内容、削除、実行属性、変更範囲、ツールの版と設定で識別する。同じ識別値の合格は再利用する。検査選択・見送りの `VERIFY_ARGS` はビルドを変えないため識別から除くが、並列数など他の Make フラグは保持する。ソースやツールが変われば合格も見送りも無効になり、必要な理由は再記録する。`--force` は同じ識別値でも再実行する。外部の可変入力を使う新しい検査は、その入力も識別へ含める。

検査中に編集を検出すると結果は無効になる。`target/verification/.lock` の排他ロックを保持する間は編集を待つ。ファイルはロック解除後も残るので、存在するだけで「実行中」と判定しない。ロックファイルを削除して並行実行を許してはいけない。`make doctor` はロックの保持状態を確認する。

## 環境、フックと CI

新しい環境では `make setup`、動的検査には `make setup-bridge`。製品のコンパイラーは `rust-toolchain.toml`、動的検査だけは `scripts/bridge-toolchain.env` の固定 nightly。ASan の言語別実行と TSan の同一プロセス実行の理由は [ADR 0044](decisions/0044-swift-bridge.md)。自動検査の環境不足も合格ではない。ソケットやキーチェーンが実行環境に拒否された場合は、許可された環境で再実行する。

`.cursor/hooks.json` の preToolUse は変更するツールをロック解除まで待たせ、stop は completed イベントで同じ検査器を呼ぶ。失敗・理由のない未実行は続行指示を返す。JSON の入力不良も黙って合格にしない。フックのない環境でも `make verify` は必須。

`.codex/hooks.json` とそのアダプターも存在するが、イベント名・設定形式は `.cursor` と異なる。設定ファイルがあることだけでは現在のホストで有効とは判定できない。直接のプロトコル検査、ホストの実イベントの発火、リモート CI の実行を別々に報告する。ホスト側の Hooks 設定・出力で発火を確認するまでは、終了検査が自動で働いたと宣言しない。

CI の自動検査一覧は JSON から生成する。各ジョブは `--all --only ID` で分担し、advisories は別に明示実行する。ローカルの `make verify` の合格を GitHub ランナー上の合格へ読み替えない。調査資料の収集は [デバッグ手順](debugging.md) を参照する。
