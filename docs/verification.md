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

## 事前確認と実行順序

`make verify` は選択と合格再利用の判定後、実行予定の検査が宣言した `preflight` を重複なく実行する。
ツールチェーン、オフライン依存、固定 nightly の rust-src / Miri、TSan の準備状態を軽量に確認する。
対象外・合格再利用・理由付き見送りの検査には準備を要求せず、取得・ビルド・機器操作もしない。
失敗時は長い検査を開始せず、`report.md` と `preflight-<ID>.log` に環境エラーと復旧案内を残す。
事前確認は製品検査の合格ではなく、既存の `check-tsan-runtime` 等も省略しない。

検査は JSON の `priority` と `after` に従う安定した順序で実行する。
フォーマット、ポリシー整合、TSan ランタイムの対照、選択された ASan を高コストな製品契約より先に確認する。
`after` は対象集合内の先行関係だけを指定し、検査の追加選択・省略・並列実行はしない。
開始から最初の失敗までの時間と対象は `report.json` の `execution.first_failure` に記録する。

`make spike-verification-failures` は同じ集合へ失敗を注入する比較計測で、結果は `target/verification/failure-latency.json`。
通常候補には含めず、`--run spike-verification-failures` で明示できる。
製品コード・機器を動かさない模擬計測であり、実際の ASan ビルド時間や過去セッションの再現とは区別する。
監査と制約は [ADR 0166](decisions/0166-verification-failure-detection.md) を参照する。

## 作業中の個別コマンド

単独の検査は直接実行して終了コードを保つ。
独立した複数検査を全件実行し、出力をログへ抑える場合は次を使う。

```sh
python3 -B scripts/checked_commands.py --log-dir target/focused-checks \
  --command 'cargo fmt --all --check' \
  --command 'cargo test -q -p echode-model'
```

`--command` は引用した argv でありシェル構文ではない。`|`、`;`、リダイレクト、`bash -c` で包まない。
一つでも失敗すれば全体も非ゼロで終了し、完全ログと `commands.json` に各終了コード・時間を残す。
ログの絞込みは実行後に別の読込みで行い、検査の成否を表示コマンドの成否へ置き換えない。
TDD の意図した red だけは、1 コマンドへ `--expect-exit` と `--expect-failure` を両方指定する。
後者はその回帰固有のアサーションの正規表現とし、単なる `failed` やビルドエラーを条件にしない。
時間切れ・起動不能・シグナル・別の終了コードは期待した red にならない。
これはログ用の補助で、検査の選択・キャッシュ・完了判定は引き続き `make verify` だけが担当する。

未コミットの変更が複数の作業にまたがっていても、各検査の依存入力が前回と同じなら結果を再利用する。
候補を一律に `--run` せず、今回の影響・過去の有効な結果・検査の目的を確認する。

## 自動検査

通常の `make check` は `check-format`、`check-verification`、`check-base`、`check-product-contracts` を順に 1 回ずつ実行する。
`scripts/verify.py` では JSON の `make_target` により、結果 ID `check` が `check-base` を実行し、基盤と製品契約は別の結果として記録する。

- `check-verification`: 検査選択、依存入力の失効、診断、署名、作業資料のリンクと行長整合、ADR・監査・スパイクを含む全文書のローカルリンク。
  既存のローカル証拠は文書・宛先の例外一覧と注意書きを要求し、新しい `target/` リンクは拒否する（[証拠の保全](evidence/README.md)）。
  macOS 27 / M4 のビルド基準と生成物も検査する（[ADR 0114](decisions/0114-m4-build-baseline.md)）。
  文書だけの変更ではこれを再実行し、入力が変わっていない製品検査は再利用する。
  エージェント入口の選択の一致・失敗情報の保持・原文読込と鮮度の境界も既存の検査内で確認する（[手順](agent-workflow.md)）。
- `check-format`: 従来 `check-base` のビルド後に行っていた Rust フォーマットを独立して先に検査する。
- `check` (`check-base`): Rust / Swift のビルドと全テスト、ランダムな編集の不変条件・Undo / Redo・保存往復、基準音声、保存形式、ブロック長比較、プラグインのクラッシュ・ハング、clippy、FFI、ライセンス。
  `make check-swift-actions` で登録簿・公開スキーマと Swift の生成編集型を照合し、古い生成物を変更せず拒否する。
- `check-product-contracts`: [scripts/verification.json](../scripts/verification.json) の `product_contracts` に定義された全必須契約テストを実行する。
  指定した実テストが 1 件走った証拠を要求し、削除・無視・0 件を拒否する。
  さらにソースのコピー上で各契約の `negative_control`（既知の不具合を戻す差分）を適用し、テストの期待値で失敗しなければ検査全体を失敗とする。
  コンパイル失敗は検出の証拠にしない。
  コピーは `target/product-contract-mutations/checkouts/` に残し、変わったファイルだけを新しい時刻で書き換える（[ADR 0148](decisions/0148-verification-pipeline-speed.md)）。
  Rust の反例は変えるクレートの依存の浅い順にまとめ、Swift の反例は 3 コピーで同時にビルドしてからテストを 1 件ずつ実行する。
  通常の実行は 1 契約 1 プロセスを保ち、Swift はテスト製品のビルド後に同じ SwiftPM の Testing ヘルパーを直接起動する。
  パッケージ解析・テスト探索の反復を省くが、一件判定・個別期限・反例ビルド時間の減算は維持する。
  Rust の同じ置換はソース遷移を共用し、対象の Cargo コマンドとテストプロセスは一件ずつ維持する。
  通常テストと反例は別の識別値で再利用し、契約全体の合格には両段階の合格を要求する。
  反例定義だけの変更や反例段階の中断では、入力が同じ通常テストの完了記録を保持する。
  どちらの段階も、現在リンク・実行する製品コード・テスト・補助・ツールを入力に含める（[ADR 0197](decisions/0197-verification-target-scopes.md)）。
  Swift の負の対照も契約ごとに全反例の終了直後に保存し、後続の中断で完了した契約を失わない。
  複数反例の一部だけの終了は合格記録にしない。
  各契約の ID・対象テスト・目的・負の対照は `scripts/verification.json` を正本とし、本書へ重複記載しない（[ADR 0093](decisions/0093-product-contract-gate.md)、[ADR 0119](decisions/0119-agent-token-efficiency.md)）。
- `rtsan`: 同梱 Sine Pad の隔離処理・領域拡張、テスト用 DSP、計画の受け渡し、製品の再生・トランスポート・出力バッファ、プラグインヘルパーの処理区間でリアルタイム処理の禁止操作を検出する。
  試聴の有限の先読み・音量補間・指定位置からのコールバックも対象とする。禁止操作はロック、確保、ブロックするシステムコールである。
- `check-input`: Swift の入力の単体テスト。ウィンドウは開かない。
- `check-bridge-features`: 橋渡しの全 feature、Rust テスト・clippy・生成器を含むライセンス。
- `check-tsan-runtime`: C だけのスレッド再利用、寿命修正を外した負の対照、生存中の errno を共有する実競合の検出を確認する。
- `check-bridge-asan`: 製品 ABI・音声出力バッファの Rust 検査と、Swift の編集・保存・再読込を別プロセスでメモリ検査する。
- `check-bridge-tsan`: 標準ライブラリを含む製品の Rust と Swift を、同じ固定ランタイムで競合検査する。
  読み込まれたランタイムも照合する（[ADR 0121](decisions/0121-startup-recovery-and-errno-validation.md)）。
  製品とスパイクの Swift は `-D ECHODE_TSAN` で、継続の先行再開の前後関係を TSan へ示す注釈を入れる（[ADR 0207](decisions/0207-continuation-handoff-tsan.md)）。
  対照 `continuation-*` は、注釈なしで報告が出ること、注釈で消えること、再開の後の書込みは報告されることを確認する。
  製品とスパイクの TSan は同じ `target/bridge/thread/rust` で標準ライブラリ・共通依存のビルドを再利用する。
  ASan・通常・反例とは出力を分け、異なる feature 構成を一括実行へ混ぜない（[ADR 0198](decisions/0198-verification-launch-and-cache-cost.md)）。
  ASan / TSan の Swift は復旧同意のスイートを別プロセスへ分け、全体の同時実行の負荷から隔離する。
  対象と 5 秒の待機期限は維持し、スイートの 0 件実行と失敗を拒否する（[ADR 0185](decisions/0185-piano-auto-and-fixed-grid.md)）。
- `check-bridge-miri`: Rust 側のポインタ・所有権。製品のスナップショット保持、UTF-8 の借用、可視列の容量・null を seed 1・17・42 で検査する。
  フォルダ開閉用の表示ハンドルの保持・再展開と可視行の容量境界も同じ経路で確認する。

## 実機・計測・ネットワークの検査

実機・聴感の判定は原則としてユーザーが担当する。
実機を使う検査は明示的な依頼がある時だけ `--run ID` で実行し、ユーザーが担当する項目は理由付きで `--defer ID --reason 理由` で見送る。
ウィンドウを使う検査は現在の Space で表示条件を満たせる環境で実行する。

- `spike-isolation`: 隔離の release 計測と製品 CLAP の実時間ブロック期限。明示的な依頼がある時に実行し、[S3](spikes/S3-plugin-isolation.md) の数値を更新する。
- `spike-draw`: Metal の大規模シーンと可視判定の実機計測（[S1](spikes/S1-drawing.md)）。
- `spike-arrange-frame`: 画面を開かず製品の Metal 描画を読み戻し、倍率 1・2、ズーム・スクロール、幅、文字、行境界、ヘッダへのはみ出しと、入力と違う大きさへ描いた重なり画素・余白画素を比較する（`target/spike-arrange-frame.json`）。
- `spike-arrange-query`: S1 と同じ件数で製品の `ArrangeIndex` の全体・部分問い合わせを release 計測する。
  ウィンドウ・機器・GPU は使わず、`target/verification/spike-arrange-query.log` に結果を記録する（[S1](spikes/S1-drawing.md)）。
- `spike-piano-frame`: 画面を開かず製品の Metal 描画を読み戻し、鍵盤・ノートの音高行、調に依存しない白黒、ディグリー表示と、入力と違う大きさへの描画を比較する。
  参照ノートの背面色・編集ノートの優先と、鍵盤・ベロシティへの非混入も倍率 1・2 で確認する（`target/spike-piano-frame.json`）。
  自動と固定の三連符・付点グリッドの細分線の画素も同じ倍率で比較し、`checkedGridLines` に採取数を記録する。
  吸着幅と独立した拍・4 拍の目印も、粗い幅と最小倍率で読み戻し、`checkedIndependentGuides` に採取数を記録する。
  両フレーム検査は描画部品の比較であり、提出・版の消費・休止・再試行は通常の必須契約で製品と同じ提出経路を検査する。
- `spike-app`: 製品のアレンジとピアノロールを実表示し、メインスレッドの 0.5 秒の負荷中も描画することとアクセシビリティ ID を確認する（`target/spike-app.json`）。
- `spike-playhead`: 専用 AppKit プローブ（`EchodePlayheadProbe`）で実ウィンドウのプレイヘッド移動・再生・一時停止・休止・破棄・文書交換を確認する（`target/spike-playhead.json`）。
- `spike-history`: 大きい文書の保存・復元と、無関係なノートを増やしたゲイン差分の release 計測（[S4](spikes/S4-history.md)）。
- `spike-input`: 確認ウィンドウでピンチ・触覚・感圧を実機確認する（[S5](spikes/S5-input.md)）。
- `spike-engine-smoke`: 現在の音声機器のレートで 1,000 ノードの処理を 10 秒確認する（`target/spike-s2-smoke.txt`）。
- `spike-engine`: 明示実行用の 10 分計測（600 秒）と逆順イベント整列の release 計測（[S2](spikes/S2-engine.md)、[ADR 0096](decisions/0096-measurement-completion.md)）。途中停止・機器変更・中断は不合格とする。
- `spike-audition`: 機器を開かず製品の試聴スレッドとコールバックで、4 秒・60 秒の素材の開始を release 計測する。
  最初の読込み・変換・受渡し・発音の時刻と、全復号前に発音したことを `target/spike-audition.json` へ記録する。
  実機の出力待ちと聴感の計測とは区別する。
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
契約では全契約共通のプラグイン依存を置かず、実バンドルを使う Rust パッケージの追加入力を `rust:<package>` へ宣言する。
ライブラリとして使う依存先の標準 `tests/**`・`benches/**`・`examples/**` は、そのライブラリの入力に含めない。
`src/**` の単体テスト、マニフェスト、素材は保持し、build script・custom target のあるパッケージは全入力を保持する。
橋渡しの ASan・TSan・Miri も `package_scope: targets` でこの境界を使う。
統合テストも実行するパッケージは `test_packages`、テストディレクトリから読む fixture は明示的な `inputs` に宣言する。
明示した入力はターゲットの除外より優先する。通常の workspace 検査の入力は縮めない。
Makefile はファイル全体ではなく、その検査のレシピ（`$(MAKE)` で呼ぶ対象を含む）と、変数などの全体に効く行だけを識別に含める。コメントの編集では失効しない。
`scripts/verification.json` と Makefile の変更は、差分の基準と比べて定義・レシピが変わった検査だけを選ぶ。基準を読めない時は全自動検査を選ぶ。
`evidence_inputs` の共通実行器の変更は全自動検査を選ぶ。
診断・表示・フック・CI の編集は基盤検査が扱い、製品検査の全件選択や共通識別値の失効理由にはしない。
Markdown と `--plan` の表示は `scripts/verification_display.py`、時間診断は `scripts/verification_metrics.py` に分離する。
自動検査は自身の `inputs` に一致する変更でも選ばれ、補助スクリプトの変更はそれを実行する検査を選ぶ。
`--force` は検査内の契約ごとの再利用も行わず、全契約と反例を実行する。
`tool_inputs` に指定した検査専用ランタイムは、Git の対象外でも実バイトをハッシュ化する。
文書だけの変更は製品コードの検査結果を失効させない。
`device_context` を持つ実機計測は、`system_profiler` で読み取った既定出力デバイスのメタデータのハッシュも識別に含める。
`target/verification/.lock` の排他ロックを保持する間はファイル変更を待つ。`make doctor` はロックの保持状態と各検査結果の鮮度（`current` / `partial` / `stale`）を確認する。

## 実行時間の保守

`make verification-profile` は検査を動かさず、既存の実行時間と契約数・反例数、段階別の時間、直近の履歴を表示する。
`report.md` / `report.json` の `execution` は今回実行した検査と再利用を区別し、再利用した過去の秒数を今回の合計へ加えない。
完了した呼出しは `target/verification/timing-history.jsonl` へ追記する。
ソース・ツールの識別値、実行集合、契約数も残すが、履歴自体は合格の根拠にも再利用キャッシュにもしない。
旧形式や中断した `running` は、全体の時間を測り終えた記録と扱わない。
開始・終了時の CPU 数・負荷平均・空き容量も記録するが、負荷の変化で合格を失効させない。
一括予算の変更だけでは個別契約の合格を失効させず、個別の期限・入力・実行経路の変更は失効させる。
反例の途中で上限に達した時も `negative_progress` に完了数と経過秒を残す。
通常テストと反例の `normal_pending`・`negative_pending`、`reused_tests`・`reused_controls` も分けて記録する。
Rust の製品コードの変更は Swift のリンク先の変更でもあり、通常・反例とも再検査する。
Swift パッケージ内のテスト間の依存分割は未実施で、Swift テストの局所編集による全 UI 契約の失効は残る。

遅くなった時は、まず次を比較する（[ADR 0195](decisions/0195-verification-cost-observability.md)）。

1. 再利用が減ったのか、同じ検査の実行自体が遅くなったのかを分ける。
   `make verify-plan` の集合と、履歴のソース・ツール・契約数を確認する。
2. 製品契約では `product-contracts.json` の `execution` を読む。
   通常実行、コピー同期、Rust / Swift 反例、実ビルド数・反例数と直列テスト秒を分ける。
   並列の `build_work_seconds` は作業秒の和であり、段階全体の実測 `seconds` とは足し合わせない。
   Rust の `source_transitions` と `test_processes` は同一置換のビルド共用と全反例の独立実行を区別する。
3. 同じ機械・入力・対象集合・ウォーム状態で比較する。
   必要なら `python3 -B scripts/verify.py --all --only check-product-contracts --force` で全契約を測り直す。
   これは作業中の個別計測であり、完了時の `make verify` の代わりではない。
4. 件数増加、再ビルド範囲、機器・外部環境の変化を識別し、期待値・期限・隔離を緩めずに直す。
   原因と比較条件・数値は追跡する ADR 等へ要約する。作業用の全ログを恒久的な証拠にしない。

Swift の `build_product` はポリシーに宣言し、製品契約ではテスト製品だけをビルドする。
同一反例はビルドのみを共用し、すべての契約・反例のテストを独立プロセスと従来の期限で判定する。
異なる反例を同じコピーへ混ぜず、全ビルドが終わってからテストする。
アプリとプローブのビルド範囲は通常の `check-base` で維持する。

## 環境、フックと CI

新しい環境では `make setup`、動的検査には `make setup-bridge` を実行する。
製品のコンパイラーは `rust-toolchain.toml`、橋渡しの動的検査（ASan / TSan / Miri）は `scripts/bridge-toolchain.env` の固定 nightly を使う（[ADR 0044](decisions/0044-swift-bridge.md)）。
TSan のセットアップには CMake と Ninja も必要で、固定した公式 LLVM のソースを取得する。
検査専用ランタイムの準備・再生成は `make setup-bridge`、取得済みソースだけの再ビルドは `python3 scripts/tsan_runtime.py --prepare` を使う。
Xcode や製品アプリは変更しない。通常検査はネットワークを使わず、古い準備は失敗として報告する。

`.cursor/hooks.json` の `preToolUse` は編集ツールをロック解除まで待たせ、`stop` は `completed` イベントで `scripts/verify.py` を呼ぶ。
`.codex/hooks.json` もアダプターを備えるが、設定ファイルの存在だけでホスト側のフック発火済みとは扱わない。フックのない環境でも `make verify` は必須である。
フックの上限は `verification.json` の `hook_timeout` と一致させ、全自動検査と事前確認の直列予算を覆うことを検査する。
ロック待機の上限は同じ予算から JSON 応答用の 10 秒を引く。これは検査自体の高速化や個別期限の延長ではない。
CI の自動検査一覧は `scripts/verification.json` から生成し、各ジョブが `--all --only ID` で分担する。調査資料の収集は [デバッグ手順](debugging.md) を参照する。
