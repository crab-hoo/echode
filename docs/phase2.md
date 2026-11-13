# Phase 2 の進め方

開始: 2026-10-17。P1 の根拠は[最終照合](reviews/2026-10-17-phase1-acceptance.md)を保持する。
Phase 2 全体の範囲と完了条件の正本は [PLAN の Phase 2](PLAN.md)。
この文書は依存関係に沿った実装順と、各単位を完了にするための受入条件をまとめる。

- **現在の進捗**:
  - **P2-01（ヘッドレス MCP）**: 実装と自動受入を完了（[ADR 0115](decisions/0115-phase2-mcp.md)）。外部クライアント設定・GUI 接続・実機確認は別途扱う。
  - **P2-02（デバイスビューとチェイン操作）**: 着手中。トラックの FX ボタン、下部の FX ビュー、非モーダルの候補パネル、並べ替え・複製・別トラックへのコピーを接続済み（[ADR 0118](decisions/0118-track-fx-chain-editing.md)）。
    Sine Pad を明示的な同梱 CLAP とし、通常のデバイス操作と隔離へ統合した（[ADR 0123](decisions/0123-explicit-sine-pad-plugin.md)）。
    CLAP のホストバイパスを保存・Undo・再生・書出しへ接続した（[ADR 0120](decisions/0120-device-bypass.md)）。
    CLAP の汎用パラメーター欄と、プラグイン状態としての単一 Undo を接続した（[ADR 0130](decisions/0130-clap-parameter-editing.md)）。
    下部を横並びのデバイスビューへ改め、画面内の行だけを作る一覧・検索・ピン留めと、ワーカー外のデバイス要求を接続した
    （[ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)）。
    ヘルパー所有のプラグイン独自ウィンドウと、プラグイン側の値変更の中継・単一 Undo を接続した
    （[ADR 0132](decisions/0132-helper-plugin-windows.md)）。
    共通サンプルブラウザの選択時の単発試聴を接続した（[ADR 0135](decisions/0135-sample-audition.md)）。
    停止中の最初のデバイス表示・再生で、出力とヘルパーの準備を文書ワーカーの外へ移した（[ADR 0136](decisions/0136-output-opening-off-worker.md)）。
  - **操作の要望の修正（UIF-01〜10）**: 2026-10-22 に操作を決めた。順番・場所・回帰・受入は
    [照合の「修正の進め方」](reviews/2026-10-21-ui-feedback-audit.md#修正の進め方) に置く。
    1 番目のブラウザのクリック経路と試聴の診断を接続した（[ADR 0143](decisions/0143-browser-click-and-audition-timing.md)）。
    2 番目のペインのリサイズで前のフレームを伸縮しない配置と、指定ヘッダ幅の保持を接続した（[ADR 0144](decisions/0144-unscaled-canvas-resizing.md)）。
    3 番目の制作向けファイルの表示と音声の位置指定ドロップを、単一 Undo・保存・失敗時の巻戻しへ接続した（[ADR 0145](decisions/0145-sample-browser-file-drops.md)）。
    5 番目の下部の個別開閉・固定高のデバイス・アレンジ非表示と旧設定の移行を接続した（[ADR 0146](decisions/0146-independent-editor-stack.md)）。
    4 番目のうち、少しずつの復号・SRC と同じ読込みからの部分波形を接続した
    （[ADR 0151](decisions/0151-streaming-sample-audition.md)）。
    表示中の出力保持・音量・自動試聴の切替・右矢印と波形クリックの手動試聴も接続した
    （[ADR 0152](decisions/0152-browser-audition-controls.md)）。実機の操作感と聴感はユーザー確認待ちである。
    6 番目の値の操作子は、相対ドラッグ・ダブルクリックの既定値・トリプルクリックの入力欄と、CLAP の `text_to_value` による表示単位の入力を接続した
    （[ADR 0153](decisions/0153-value-control-clicks-and-typed-entry.md)）。
    7 番目のクリップの移動（UIF-03）は、P2-07 のアレンジ編集の先行分として、本体ドラッグの移動・Command の複製移動を接続した。
    移動先で重なる配置は置き換え、複製の中身は元と独立する（[ADR 0154](decisions/0154-clip-move-and-duplicate.md)）。
    UIF-08 は P2-02 の下部の切替を置き換えた。
  - **P2-07 のトランスポート**: 記憶した再生開始位置、停止での復帰と一時停止の分離、Space の再生／停止、
    ピアノロールのルーラーのクリックとパッケージへの保存を接続した（[ADR 0155](decisions/0155-remembered-playback-start.md)）。
    ピアノロールのルーラードラッグによる範囲指定と、ループ前からの進入・終端以降からの通常進行も接続した
    （[ADR 0156](decisions/0156-ruler-loop-range-and-entry.md)）。
    アレンジのルーラーも接続し、開始位置と範囲を共有する（[ADR 0157](decisions/0157-arrange-ruler.md)）。
    既存ループ端の調整も両エディタへ接続した（[ADR 0158](decisions/0158-ruler-loop-edge-handles.md)）。
    再生ヘッドへの追従と手動スクロールでの解除も接続した（[ADR 0159](decisions/0159-playhead-follow.md)）。Return の一時停止／再開も接続した（[ADR 0181](decisions/0181-piano-reference-workflow-and-keys.md)）。
    単一ノートのドラッグは Shift の吸着解除・グリッド未満の伸縮と解放時の単一確定を接続した
    （[ADR 0167](decisions/0167-piano-note-drag-snapping.md)）。
  - **GUI のがくつき（JNK-01〜07）**: 2026-10-22 の監査で原因を分けた。
    単位 1（反転した表示面の固定位置）、単位 2（状態行・再生ボタンの寸法）、
    単位 3（ピアノロールのアクセシビリティ要素）、単位 5（アレンジの指定縦位置）を接続した
    （[ADR 0149](decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)）。
    単位 4（サイズ不一致の drawable への描画）は、入力と同じ画素位置での描画・入力の外の背景保持・
    行ラベルを含む入力境界のクリップ・版を消費しない再試行・レイヤー背景色を接続した（[ADR 0150](decisions/0150-stale-drawable-frame.md)）。
    実画面のちらつきの有無はユーザー確認待ちである。単位 6（分割線ドラッグの計測）は未着手である
    （[監査](reviews/2026-10-22-gui-jank-audit.md)）。
  - **2026-10-25 の報告（FB-01〜08）**: [照合](reviews/2026-10-25-user-feedback.md) に再現と原因を置く。
    開始位置の送信中の暗転とサンプル一覧の Space（[ADR 0160](decisions/0160-ordered-transport-requests.md)）、
    置いた音声の自動フェード（[ADR 0161](decisions/0161-placed-audio-without-fades.md)）、
    FX 候補の一覧の重さとダブルクリック（[ADR 0162](decisions/0162-plugin-candidate-table.md)）、
    ブラウザの全体の波形（[ADR 0163](decisions/0163-sample-overview.md)）、
    ノートの終端からの再生で鳴り続ける音（[ADR 0164](decisions/0164-note-end-chase.md)）を修正した。
    追加の要望 2 件は、インスペクターの FX 追加を P2-02、アレンジの MIDI クリップの音高表示を P2-07 の要件にした
    （[ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。実画面・聴感はユーザー確認待ちである。
- **操作要件の正本**:
  コード→メロディ→ドラム→ベース→構成→編曲→ミックス・オートメーション→書出しまでの操作要件は [制作ワークフローとインタラクション](interaction-design.md)、既存設計との置換範囲は [ADR 0117](decisions/0117-workflow-interactions.md) に置く。
  ループ素材の BPM 推定・テンポ追従・ストレッチ方式切替は後続 Phase の範囲とする。

## 引き継ぐ条件

- macOS 27 以降、M4 ファミリー以降（[ADR 0114](decisions/0114-m4-build-baseline.md)）。
- 各単位を製品の入口から保存・復旧・Undo / Redo・再生／書出しへつなぎ、必要な資料・回帰・診断を同時に更新する。音に関わる単位はブロック長比較と RT の検査を持つ。
- プラグインは本体外、RT の確保・ロック・待機は禁止。固定したトラック数やチャンネル数、階層の深さを上限にしない。
- 実機・聴感はユーザー担当。未解決の HAL / errno の TSan 条件は [検査手順](verification.md#検査基盤で区別すること) に残す。
- 通常の確認は `make verify`、単位の受入完了の前に `make verify-full`。各単位の受入完了と Phase 2 全体の完了を区別する。
- 監査の残りは [ADR 0278](decisions/0278-audit-backlog-order.md) の順に行い、堅牢性の R 群と保守性の P2 を終えるまで新しい単位に着手しない。
  機能に付随する項目（[統合表](reviews/2026-11-08-open-items-register.md) の F1〜F13）は、該当する単位の受入条件に含める。

## 実装順と受入条件

### P2-01: MCP から既存の編集経路を検査する（完了）

- [ADR 0115](decisions/0115-phase2-mcp.md) のヘッドレス版（`echode mcp SOCKET` → `echode serve`）を実装済み。
- アクション登録簿のツール、一括編集（`actions.transaction`）、単一 Undo / Redo、概要とトラックのリソースを提供し、`DocumentHost` とジャーナルを共有する。
- 受入（必須契約 `mcp-edit`）: 一括編集、Undo / Redo、失敗時の巻戻し、別クライアントへの公開、強制終了後の復旧、stdout 汚染の拒否を確認済み。GUI 接続や再生操作は含めない。

### P2-02: デバイスビューとパラメーター編集（着手中）

- **実装済み（[ADR 0118](decisions/0118-track-fx-chain-editing.md)）**:
  トラックヘッダの FX ボタン、下部のトラック FX ビュー、非モーダルの候補パネル、並べ替え・Command 複製・別トラックへのコピー・削除・対象トラックへの追加、単一 Undo / Redo と保存・再読込（必須契約 `fx-chain-ui`）。
- **ホストバイパス（[ADR 0120](decisions/0120-device-bypass.md)）**:
  FX 行の切替を保存・Undo / Redo・複製へ接続した。FX のドライ音、音源の無音、配置音声の保持と再有効化を製品の再生・書出し経路で検査する。
- **共通サンプルブラウザの検索・配置（[ADR 0122](decisions/0122-sample-library-search-and-placement.md)）**:
  フォルダ登録・解除と設定保持、登録素材全体の名前検索、操作時のライブ位置と対象を保持する配置を接続した。
  検索・配置の必須契約で、重複・リンク・パッケージ除外、保存・Undo / Redo と古い文書の拒否を確認する。
  一覧・登録検索の制作向けファイルへの絞込み、未対応 MIDI の表示、音声 1 ファイルの位置指定ドロップも接続した。
  拍グリッド・Shift 解除・末尾トラックの単一 Undo と、取込み失敗時の巻戻しを持つ（[ADR 0145](decisions/0145-sample-browser-file-drops.md)）。
- **全選択トラックへの一括追加（[ADR 0124](decisions/0124-selected-track-fx-batches.md)）**:
  ヘッダの Command / Shift 選択と候補の右メニューを接続した。
  操作時の対象を保持し、別デバイス ID の生成・単一 Undo / Redo・保存再読込を必須契約で確認する。
- **デバイスのオフライン化（[ADR 0125](decisions/0125-device-offline.md)）**:
  記録済み設定とバイパス値を保持し、実行グラフからの除去・再有効化時の再生成・単一 Undo・保存へ接続した。
  未検出のオフライン FX を含む実 CLI 書出しと、ブロック長・RT の回帰を必須契約で確認する。
- **トラック追加位置（[ADR 0127](decisions/0127-positioned-track-insertion.md)）**:
  コマンドは選択の下、一覧末尾のボタンは最上位末尾へ空トラックを追加する。
  展開フォルダは最後の子、折りたたみ中は部分木の直後の外側とし、操作時の対象保持・単一 Undo / Redo・保存再読込を接続した。
  空トラックで候補を自動表示しない。開閉は一時的な表示設定で、行・選択・FX ドロップ先を共通化した
  （[ADR 0128](decisions/0128-folder-disclosure-and-row-projection.md)）。
- **トラックの通常複製（[ADR 0129](decisions/0129-track-duplication.md)）**:
  選択の部分木を元の直後へ複製し、音源・FX・クリップ・ミキサー値を写して中身とノートを独立させた。
  メニュー・パレット・ヘッダの右メニューから操作時の対象を保持し、単一 Undo / Redo・保存再読込・書出しの一致を必須契約で確認する。
- **CLAP のパラメーター編集（[ADR 0130](decisions/0130-clap-parameter-editing.md)）**:
  FX 行の汎用パラメーター欄から一覧・ドラッグ中の試聴・終了時の単一 Undo を接続した。値はプラグイン状態として記録し、Undo / Redo は同じヘルパーへ戻す。
  必須契約 `plugin-params` は製品エンジンと実ヘルパー、`fx-params-ui` は実スライダーと履歴を確認する。
  欄の単クリックは値を変えず、ダブルクリックで既定値、トリプルクリックでプラグインの表示単位の入力欄を開く
  （[ADR 0153](decisions/0153-value-control-clicks-and-typed-entry.md)、必須契約 `fx-params-clicks-ui`・`plugin-text-parse`）。
- **横並びのデバイスビューと応答性（[ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)）**:
  選択トラックのデバイスを横に並べ、各列に操作・検索欄・全パラメーターの一覧を置いた。一覧は画面内の行だけを作る。
  ピン留めはデバイスごとに保存形式 6 へ記録し、最上部に固定する。単一 Undo・複製時の継承・検索中の表示を持つ。
  下部は [ADR 0146](decisions/0146-independent-editor-stack.md) で個別開閉へ改め、デバイスを固定高でピアノロールの下へ積む。
  ⌥⌘3・4 は各エディタの開閉、⌥⌘5 は開閉と独立したアレンジ非表示。v1 のウィンドウ設定を v2 へ引き継ぐ。
  一覧・値の往復はデバイスレーンで待ち、選択・無関係な編集・Undo・停止は応答を待たない。
  必須契約 `device-lane-held`・`device-view-slow-ui`・`device-view-long-ui` で確認する。
- **サンプルの単発試聴（[ADR 0135](decisions/0135-sample-audition.md)）**:
  ブラウザの選択で音声素材を素の音のまま先頭から 1 回鳴らし、フォルダ・非音声・選択解除で止める。
  新しい選択は先頭から鳴らし、前の音は 5 ms で消す。要求は文書ワーカーを通らず、専用の IOProc で曲の再生に重ねる。
  必須契約 `sample-audition` は実際の復号・試聴スレッド・コールバック、`sample-audition-ui` は選択の扱いを確認する。
  ブロック長比較と RTSan の回帰を持つ。2 つの IOProc の加算と聴感はユーザー確認とする。
  一覧は押した時点で選択し、試聴は利用者のクリックとキーだけで変わる。要求ごとの各段の時刻を記録する
  （[ADR 0143](decisions/0143-browser-click-and-audition-timing.md)、必須契約 `sample-browser-click-ui`）。
  長い素材は有限の先読みリングで全復号前に鳴らし、同じ読込みの波形・長さ・読込み状態を一覧の下へ表示する。
  選択変更後の古い波形の拒否、読込み中の停止、不足からの再開と EOF を回帰で確認する
  （[ADR 0151](decisions/0151-streaming-sample-audition.md)、必須契約 `sample-audition-stream`・`sample-preview-ui`）。
  表示中は無音の出力を保持し、試聴だけの音量を 5 ms で補間する。
  自動試聴の切替・右矢印・再生／停止ボタン・波形クリックの元サンプル位置からの試聴を接続した。
  設定はプロジェクトの履歴から独立する（[ADR 0152](decisions/0152-browser-audition-controls.md)）。
  一覧の下の波形は、選択で素材の全体を試聴と別に読み、停止後も保ち、直近の素材は読み直さない
  （[ADR 0163](decisions/0163-sample-overview.md)、必須契約 `sample-overview`・`sample-overview-ui`）。
  一覧にフォーカスがある時の Space は曲の再生／停止である（[ADR 0160](decisions/0160-ordered-transport-requests.md)、必須契約 `sample-list-space-ui`）。
  置いた音声には自動のフェードを付けない（[ADR 0161](decisions/0161-placed-audio-without-fades.md)、必須契約 `journal-place-fades`）。
- **ヘルパー所有のプラグインウィンドウ（[ADR 0132](decisions/0132-helper-plugin-windows.md)）**:
  列の名前・インスペクターの名前のクリックと候補からの単体追加で開き、一括追加・コピー・トラック複製では開かない。
  開く・前面化はフォーカスを取らず、Echode が非アクティブの間は隠す。
  プラグイン側のジェスチャーは終了時に 1 回の Undo、窓が開いている間のジェスチャーなしの変更は静止後に記録する。
  必須契約 `plugin-window-helper`・`plugin-window-relay` は実ヘルパー、`plugin-window-undo-ui`・`plugin-window-open-ui` は画面と履歴を確認する。
- **一覧の失敗からの再試行（[ADR 0140](decisions/0140-device-parameter-list-retry.md)）**:
  失敗したデバイス列の再読込みボタンから、その一覧だけを再試行する。読込み中は連打を集約し、他の列・編集・停止を止めない。
  必須契約 `device-list-retry-ui` は実ボタンと応答の保留で、検索・履歴・選択の保持、再失敗からの復帰を確認する。
  単体回帰でオフライン・削除・旧文書の拒否と、遅れた再試行の応答を捨てることも確認する。
- **デバイス列の開閉（[ADR 0137](decisions/0137-device-disclosure.md)）**:
  開閉ボタンで名前と状態・チェイン操作を残して列を縮め、検索・一覧の状態を保持する。
  文書内の表示設定としてトラック切替や列の再生成でも維持し、保存・Undo・プラグイン通信を変更しない。
  必須契約 `device-disclosure-ui` は実ボタンと、可変幅の全境界での移動・コピードロップ先を確認する。
- **独自ウィンドウの表示状態（[ADR 0138](decisions/0138-plugin-window-status.md)）**:
  デバイス列とインスペクターの名前の横に窓のアイコンを表示する。一時非表示も開として扱い、編集時の過去の状態とは別の寿命通知で更新する。
  必須契約 `plugin-window-state` は制御ソケット、`plugin-window-status-ui` は実際の列と保存・Undo の対象外を確認する。
- **トラックヘッダの FX の名前（[ADR 0139](decisions/0139-track-header-fx-names.md)）**:
  先頭の挿入済み名前から独自 GUI を開き、右の「…」で残りのチェインを選ぶ。FX 追加ボタンは従来の候補を開く。
  必須契約 `fx-header-ui` は名前・追加入口の分離、メニュー・アクセシビリティと古い対象の拒否を確認する。
- **FX 候補の一覧（[ADR 0162](decisions/0162-plugin-candidate-table.md)）**:
  AppKit の表で押した時点で選択し、行のどこでも 2 回目のクリックで追加する。再走査中も表示中の候補を追加できる。
  必須契約 `plugin-list-ui` で確認する。
- **インスペクターの FX 追加（[ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）**:
  プラグインの一覧末尾の FX 追加ボタンから、そのトラックを入力先に既存の候補パネルを開く。空のチェインでも表示する。
  必須契約 `fx-inspector-ui` は実 View のボタンから文書と入力先、履歴不変・旧文書の拒否・無効なボタンを確認する。
  実画面と VoiceOver はユーザー確認待ちである。
- **残る範囲（[操作要件](interaction-design.md#fx-候補とチェインの操作)）**:
  - 複数選択時の表示対象は、選択の中のフォーカスしたトラックとし、接続済みである（`TrackSelection.focus`、`trackSelectionKeepsRangeAnchorAndSingleInputFocus`）。
    マクロは既存の Phase 3 の計画で扱い、パラメーターのピン留めをマクロの実装とはしない。
  - サムネイル。
    実ウィンドウ・第三者プラグインの GUI はユーザー確認とする（[ADR 0132](decisions/0132-helper-plugin-windows.md)）。
  - フォルダ型ドラムラック（[ADR 0116](decisions/0116-folder-drum-rack.md)）：サンプルのドロップ、子トラックとサンプラーの作成、既存行の差替え。
  - GUI アプリの MCP 入口は同じ書き手を共有し、別の `DocumentHost` でパッケージを開き直さない。
- **応答性の追加受入**:
  [処理中も作業を続ける](interaction-design.md#処理中も作業を続ける)を P2-02 の完了条件に含める。
  - 接続済み（[ADR 0131](decisions/0131-device-view-and-responsive-parameters.md)）:
    テスト用 CLAP の一覧をヘルパー内で保留し、製品エンジンの制御側が背景保存・グラフ交換・適用済み状態の読取りを止めないことを確認する。
    画面側は応答の保留中に別トラックの選択・無関係な編集・Undo・停止・他デバイスの一覧が終わることを確認する。
    遅い応答が新しい一覧・削除済みデバイス・閉じた文書を上書きしないこと、3000 個のパラメーターで画面内の行だけを作ることも確認する。
    画面外・検索・ピン留めからの値の変更が正しいデバイスとパラメーター ID へ届くこと、ピン留めの固定・独立・保存再読込を確認する。
  - 接続済み（[ADR 0136](decisions/0136-output-opening-off-worker.md)）:
    停止中の最初のデバイス表示と最初の再生は、ヘルパーの起動と素材の準備を準備スレッドで行い、文書ワーカーは後続の編集・Undo・停止を続ける。
    待つのはデバイス要求だけで、投入順に準備後のレーンへ渡す。準備中の編集・位置・ループ・再生／停止は導入時に反映する。
    必須契約 `output-opening` は製品のコールバック、`output-opening-worker` は C ABI からワーカーの手番を確認する。
  - 接続済み（[ADR 0141](decisions/0141-device-output-preparation-status.md)）:
    未取得の列は出力の準備中と一覧の応答待ちを区別し、取得済みの列・失敗・オフラインを優先する。
    必須契約 `device-preparation-ui` は音声状態のポーリングと実デバイス列で、準備完了後も一覧の返答を待つこと、編集・停止と検索・履歴の保持を確認する。
  - 接続済み（[ADR 0142](decisions/0142-background-plugin-graph-preparation.md)）:
    開いた出力の置換ヘルパーは準備スレッドで起動する。文書ワーカーは後続編集・Undo/Redo・停止を続け、最新の計画へ収束する。
    必須契約 `background-graph` は実ヘルパーの初期化を保留し、制御の手番と既存音声を確認する。
    既存ヘルパーの状態復元・ループ変更との競合、非同期の失敗、Session からの新デバイスの待ちも回帰で扱う。
  - 接続済み（[ADR 0147](decisions/0147-output-prewarm-and-background-media.md)）:
    製品アプリはプロジェクトを開いた時に出力を停止のまま準備し、最初の再生は保持した出力を再開する。
    停止中の編集の素材の読込み・SRC は別スレッドで行い、読込み中の再生は新しい計画まで保留する。
    必須契約 `output-prewarm`・`output-prewarm-play`・`output-prepare-worker`・`output-prepare-on-open` で確認する。
  - 残る条件:
    計画生成は文書ワーカーで待つ。テンポの変更の作り直しは、ノートの番号を位置で引き継いで約 7 倍速くした（[ADR 0335](decisions/0335-retimed-plans-keep-note-numbers.md)）。
    初回の出力導入時の差分で増えるヘルパーも準備スレッドで起動し、再生の開始はその計画の公開を待つ
    （[ADR 0333](decisions/0333-install-diff-graph-off-worker.md)、必須契約 `output-install-graph`）。
    再生中の編集の素材も別スレッドで読み、テンポの変わった計画は採用時に拍を保つ
    （[ADR 0330](decisions/0330-playing-edit-media-and-tempo-retime.md)、必須契約 `playing-edit-media`・`output-install-held-play`）。
    外部の音声の取込みは、復号と保管庫の横への書込み（段階）を別スレッドで行う。その間も音声・デバイスの要求と再生ヘッドは止まらず、
    後の編集・Undo は取込みの後に投入順で当てる（[ADR 0331](decisions/0331-off-worker-audio-import.md)、必須契約 `import-off-worker`）。
    記録済みのプラグイン状態は名前で比べ、ヘルパーを保つ編集では読まない。新しいヘルパーと状態の変わったヘルパーだけが準備スレッドで読む
    （[ADR 0329](decisions/0329-plugin-states-by-name.md)、必須契約 `plugin-state-names`）。
    閉じる・機器交換の時は、起動中のヘルパーを待たずに取り消す
    （[ADR 0334](decisions/0334-cancel-helper-starts-on-close.md)、必須契約 `helper-start-cancel`・`helper-open-cancel`・`output-close-cancel`）。
    開いた出力の更新中は、未取得の列に「音声出力を更新中」と出す（[ADR 0336](decisions/0336-device-columns-show-output-updates.md)、必須契約 `device-preparation-ui`）。
    実機の操作感と第三者プラグインでの停止の解消はユーザー確認とし、時間の実測は無視した計測として通常回帰に混ぜない。
    画面側の契約は代役のヘルパーで画面の手番を、Rust 側の契約は実ヘルパーで制御側の待ちを確認する。
    両者をつないだ実機の経路は、実機での確認の対象である。

### P2-03: ルーティング・フォルダバス・PDC

先行分として、フォルダのミュート・ソロの子孫への反映（[ADR 0244](decisions/0244-folder-mute-and-solo-gates.md)）と、
子の音を親のチェイン・音量・パンへ通すフォルダバス（[ADR 0338](decisions/0338-folder-bus.md)）を接続した。
PDC・センド・サイドチェイン・ドラム用の MIDI 配布は未接続である。
CLAP の遅延の取得とデバイス列の表示は接続済みだが、音の時刻は補正しない（[ADR 0246](decisions/0246-plugin-latency-reporting.md)）。
フォルダバスは Reaper と同じ意味にし、センド・サイドチェインより先に FCA-06 の解決として実装した（[ADR 0277](decisions/0277-reaper-style-folder-bus.md)）。

- フォルダの入れ子、センド／リターン、サイドチェーン、PDC（クリップ FX や重なる配置の合流を含む）を制御側の実行計画へ組み込む。循環の拒否、チャンネルの意味、加算順序、レイテンシ変更と計画交換を定義する。
- センドは送り元のドラッグ・一覧と受け側の一覧から同じ接続を作る（新規接続は FX 後・フェーダー前の 0 dB、元フェーダーとは独立しミュートで停止、ソロ時は関連リターンのみ聴こえ非ソロの共用送り元は鳴らさない。詳細は [センドとレシーブ](interaction-design.md#センドとレシーブ)）。
- ドラムラックでは親の MIDI を音ごとの子へ振り分け、子の音声を親へ加算する。
- 受入: 基準信号の到着位置と振幅、階層・センド・サイドチェーン・PDC、停止・ループ・シーク、ブロック長独立性、RT 禁止操作を製品バッファで確認する。

### P2-04: オートメーションとクリップ FX

- パラメーターのアドレス・モード・曲線と書込み、配置ごとのクリップ FX チェイン・状態・追従曲線・テールを実装する。最初の FX 追加でスマートクリップを固定し、リンクした中身と独立した FX の関係を守る。
- オートメーション表示は、モード有効時に全トラックのクリップ領域を曲線表示へ切り替え（最後の操作項目を自動表示）、無効時は通常のレーン（所有トラックのレーン最上部へ自動表示）とする。
- 点・線のクリックとドラッグ、右ドラッグ枠選択、グリッドと Shift、Option による曲率、手動試聴を離した時の現在時刻の曲線値への復帰を実 View から検査する（[オートメーションの表示と編集](interaction-design.md#オートメーションの表示と編集)）。
- 受入: 特定の配置だけに効くこと、状態と曲線の保持、余韻・PDC、別トラックへのバウンス取込み、Undo / Redo、保存・復旧、再生と書出しの一致を確認する。

### P2-05: VST3 / AU とブラウザの対応形式

- 共通チェインの契約へ VST3、AUv2、AUv3 を順に加える。AUv3 は OS の XPC extension にヘルパーを重ねない（[ADR 0111](decisions/0111-macos-helper-notices.md)）。
- プラグインのプリセットは、FX 候補パネルの側で検索・追加できるようにする。CLAP は preset-discovery で索引を作り、preset-load で読み込む。
  ファイルブラウザの一覧には出さない（[共通サンプルブラウザ](interaction-design.md#共通サンプルブラウザ)、2026-10-22 の回答）。
- 受入: 実形式の fixture と別プロセスを使い、追加検知・スキャン・状態・イベント・バス・GUI・再起動から製品再生・書出しまで確認する。

### P2-06: ミキサーと内蔵基本デバイス

- ルーティングとパラメーターをコンソール表示へ接続し、内蔵ユーティリティ、EQ、ダイナミクス、LUFS・トゥルーピーク等のメーターを加える。定番 DSP は GPL と両立する既存実装を調べて測る。
- 複数選択のミキサー操作は、音量は同じ dB 差、パンは同じ位置差、ミュート・ソロは同じ状態を適用し、ソロは通常クリックで加算する（[ミキサーの複数選択](interaction-design.md#ミキサーの複数選択)）。
  先行分として、ヘッダ・インスペクターのミュート／ソロの一括操作と、連打の順序・単一 Undo・保存を接続した。
  ピアノロールの行操作は単一対象を保つ（[ADR 0189](decisions/0189-selected-track-mute-and-solo.md)）。
  音量・パンの数値入力も、対象捕捉・現行値からの共通差分・上下限・単一 Undo・保存へ接続した。
  数値入力は [ADR 0191](decisions/0191-selected-track-numeric-mix.md) に定める。
  ヘッダの読み上げの増減も、現行値からの共通差分・限界・単一 Undo・保存へ接続した。
  `mixer-step-ui`・`mixer-step-bounds-ui` で連打・対象捕捉・確定値と旧文書の要素の拒否を確認する
  （[ADR 0192](decisions/0192-selected-track-accessibility-steps.md)）。
  文書交換時のヘッダ操作の取消も接続した。`mixer-gesture-document-ui`・`mixer-reset-document-ui` は旧押下・リセット待ち・数値欄が新文書へ漏れないことを確認する
  （[ADR 0193](decisions/0193-header-mix-gesture-document-scope.md)）。
  一括ドラッグ・既定値リセットも、押下時の対象・共通限界・全対象の試聴・単一 Undo・保存へ接続した。
  `mixer-drag-batch-ui`・`mixer-drag-bounds-ui`・`mixer-group-audio` で実入力と製品 PCM の取消・確定を確認する
  （[ADR 0228](decisions/0228-selected-track-live-mix.md)）。コンソール表示は未接続である。
- ドラムラックの子で使う内蔵サンプラー（ゲート再生＋音量 ADSR とワンショットの切替、再発音時の旧音リリースと新音の独立加算）と子・親のミックスを接続する（[ドラムの入力とミックス](interaction-design.md#ドラムの入力とミックス)）。
- 受入: バイパス、係数・状態更新、ステレオ、遅延、測定信号、ブロック長と RT の契約、実 View からの 1 回の Undo と保存・復旧を確認する。

### P2-07: マーカー・録音・ピアノロール

- テンポ・拍子・キーのグローバルマーカーとリージョン、MIDI / QWERTY / 外部 MIDI の録音、ループ重ね録り、単一テイクのオーディオ録音、クオンタイズ、MIDI 割当を実装する。
- **アレンジ編集（[アレンジの選択と編集](interaction-design.md#アレンジの選択と編集)）**:
  最初の MIDI クリップの音高表示は接続した（[ADR 0165](decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）。
  中身ごとの最低音〜最高音（12 半音未満は中央寄せ）で縦に置き、表示だけを変える。
  可視列・オフスクリーンの製品フレームと VoiceOver 用の局所矩形は必須契約、製品の問い合わせ時間は機器なしの `spike-arrange-query` で確認する。
  実画面・VoiceOver と S1 の実ウィンドウの再計測はユーザー担当である。
  右ドラッグの枠選択、空白左ドラッグの時間・トラック区間選択、部分コピー・空白を残す削除・現在位置と選択トラック起点の貼付け・複製、素材終端で止まる通常トリム、同トラック配置の重なりと自動クロスフェード、ループ区間からのリージョン作成とオートメーション追従を接続する（[オーディオクリップとテンポ](interaction-design.md#オーディオクリップとテンポ)、[次に詰める点](interaction-design.md#次に詰める点)）。
  クリップ本体の移動・Command の複製移動は、置き換えとして先に接続した（[ADR 0154](decisions/0154-clip-move-and-duplicate.md)、必須契約 `clip-move-ui`・`clip-move-model`）。
  単一クリップの ⌘D も、拍へ丸めない現在の窓の直後への複製・独立した中身・複製側の選択・単一 Undo と保存へ接続した
  （[ADR 0188](decisions/0188-arrange-adjacent-clip-duplication.md)）。
  `clip-duplicate-key-ui`・`clip-duplicate-key-guards-ui` は実 View の入力・履歴とフォーカス・未確定操作の拒否を確認する。
  重ねるモードの切替を接続する時に、置き換えの判定を同じ経路へ組み込む。
- **トランスポートとループ（[再生の開始位置](interaction-design.md#再生の開始位置)、[ループ再生](interaction-design.md#ループ再生)）**:
  記憶した開始位置（停止で戻る、再読込で保持、両ルーラーのクリックで変更・再生中は即時シーク）と進行中位置（一時停止でその場再開）を分ける。
  ルーラーのドラッグによるループ範囲指定（オン／オフは別操作）、ループ前からの進入とループ終端以降の通常進行を確認する。
  - 接続済み（[ADR 0155](decisions/0155-remembered-playback-start.md)）: 開始位置・停止での復帰・一時停止・Space の再生／停止、
    ピアノロールのルーラーのクリックと印、`transport.json` への保存。
    必須契約 `stop-return-tails`・`playback-start`・`playback-start-ui`・`piano-ruler-start-ui` とブロック長比較で確認する。
  - 接続済み（[ADR 0156](decisions/0156-ruler-loop-range-and-entry.md)）: ピアノロールのルーラードラッグの範囲指定。
    クリックとの分離、吸着と Shift、逆向き・取消・単一送信と、オン／オフを変えない共有の下書き・帯を持つ。
    必須契約 `piano-ruler-loop-ui` と `loop-entry` で、実 View と製品バッファの助走・終端以降の通常進行、境界の一時停止と計画交換を確認する。
  - 接続済み（[ADR 0157](decisions/0157-arrange-ruler.md)）: アレンジのルーラーと共有状態。
    四分音符吸着・Shift・単一送信・取消と、行領域の入力・描画の一致を必須契約 `arrange-ruler-ui`・`arrange-ruler-cancel-ui` で確認する。
  - 接続済み（[ADR 0158](decisions/0158-ruler-loop-edge-handles.md)）: 両ルーラーの既存端のグリップ。
    反対端とつかむ位置の保持、交差の制限、吸着・Shift、外部変更での取消、開始位置と履歴の不変を
    必須契約 `loop-handles-ui`・`loop-handles-cancel-ui` で確認する。
  - 接続済み（[ADR 0159](decisions/0159-playhead-follow.md)）: 両エディタの再生ヘッドへのページ送り、横方向の手動スクロールでの解除、
    トランスポート・再生メニュー・パレットからの再オン、押下中の保留。値は保存せず、文書を開くたびにオンで始める。
    必須契約 `playhead-follow-ui`・`playhead-follow-state-ui` で確認する。
  - 接続済み（[ADR 0160](decisions/0160-ordered-transport-requests.md)）: 開始位置・ループ・メトロノーム等の送信中も操作子を無効にせず、
    要求を捨てずに順に送る。必須契約 `transport-order-ui` で確認する。
  - 接続済み（[ADR 0164](decisions/0164-note-end-chase.md)）: ノートの終端ちょうどからの再生で、そのノートを追いかけて鳴らさない。
    必須契約 `note-end-start` で実ヘルパーと複数のブロック長を確認する。
  - 接続済み（[ADR 0181](decisions/0181-piano-reference-workflow-and-keys.md)）: Return の一時停止／再開とプラグイン窓からの転送。
    入力欄・サンプルと FX の一覧の Return はローカル操作を優先し、`piano-return-ui`・`browser-return-ui` と既存のプラグインキー契約で確認する。
- **ピアノロールとノート編集**:
  - 単一ノートの移動・左右端の伸縮は Shift で吸着を解除する。解放時の座標・修飾キーで確定し、最短長は 1 tick とする
    （[ADR 0167](decisions/0167-piano-note-drag-snapping.md)）。
    必須契約 `piano-note-drag-ui` は実 View と製品ブリッジで、単一 Undo / Redo・保存再読込、ID とベロシティ保持を確認する。
    実画面はユーザー確認待ちである。選択済み両端の一括伸縮も接続した（[ADR 0179](decisions/0179-piano-batch-note-resizing.md)）。
  - 空白の左ドラッグは、開始位置を固定して横に長さ・縦に音高を調整し、解放時に `note.insert` を 1 回送る。
    Shift は開始・終端の吸着を外すが、新規ドラッグの最短長は現在のグリッド幅とする。
    単クリックも解放時に記憶長で追加し、ダブルクリックの 2 回目は追加を重ねない。
    入力長は全トラックで共有し、選択・追加・単一ノートの長さ変更で記憶する。
    追加・長さ変更の Undo / Redo で戻し、無関係な編集の Undo では維持する。再オープン時は初期化する
    （[ADR 0172](decisions/0172-piano-click-and-input-length.md)）。
    必須契約 `piano-note-click-ui`・`piano-note-input-length-ui` は実 View と製品ブリッジで確認する。
    必須契約 `piano-note-insert-ui`・`piano-note-insert-cancel-ui` と通常回帰で、取消・仮表示・単一 Undo / Redo・保存を確認する。
  - 自動配置は、小節未満の移動・複製でも `auto` を保ち、中身の原点から数えた小節で伸縮する
    （[ADR 0170](decisions/0170-content-relative-auto-bars.md)）。
    必須契約 `clip-content-bars`・`journal-clip-bar-basis` と、実 View のノート編集で履歴・旧ジャーナル・保存を確認する。
    所属替えと壁での制限は [ADR 0169](decisions/0169-clip-growth-follows-notes-and-stops-at-walls.md) の草案に残る。
  - 編集トラックの切替とエディタの再生成で、時間・音域・倍率を共有する（[ADR 0174](decisions/0174-shared-piano-viewport.md)）。
    必須契約 `piano-shared-viewport-ui` は旧文書の書戻し拒否と文書ごとの初期化も確認する。
    アレンジ選択とは独立した単一入力先と、ダブルクリックでその位置を明示的に開く経路も接続した。
    他トラックのノートを背面へ表示し、参照だけのクリックは現在の入力先へ追加する。編集ノートとの重なりでは編集ノートを優先する。
    右側のトラックセレクターは表示・入力先と Solo/Mute を分離し、表示は文書内の一時状態、Solo/Mute は保存・Undo の対象とする
    （[ADR 0181](decisions/0181-piano-reference-workflow-and-keys.md)）。
    `piano-target-ui`・`piano-arrange-open-ui`・`piano-reference-ui`・`piano-track-selector-ui` とオフスクリーン GPU 読戻しで確認する。
    複数トラックの同時編集は残し、実画面と VoiceOver はユーザー確認待ちである（[トラックセレクター](interaction-design.md#トラックセレクター)）。
  - ノート入力・伸縮・移動：プロジェクト内で共通の入力長記憶（Undo 対象、再オープン時は現在のグリッド幅）、左ドラッグでの開始位置固定・右伸縮・上下音高変更、Shift によるグリッド吸着一時解除、右クリックメニューの対象選択、ポインター基準のピンチズームと自動／固定グリッド（[ノートの入力](interaction-design.md#ノートの入力)、[ノートの選択](interaction-design.md#ノートの選択)）。
  - ベロシティ：ノート上の Option＋上下ドラッグと下部レーンの両方から編集し、複数選択は同じ差分で相対関係を保つ（[ベロシティの編集](interaction-design.md#ベロシティの編集)）。
    単一ノート上の Option＋上下ドラッグと入力値の共有は接続した（[ADR 0173](decisions/0173-piano-note-velocity.md)）。
    `piano-note-velocity-ui`・`piano-note-velocity-cancel-ui` と `note-velocity-model` が実入力・取消・履歴・共有内容と保存を検査する。
    下部の棒は同じ編集へ接続した（[ADR 0175](decisions/0175-piano-velocity-lane.md)）。
    `piano-velocity-lane-ui`・`piano-velocity-lane-frame-ui`・`piano-velocity-lane-cancel-ui` と GPU 読戻しで確認する。
    選択全体の同量増減も、両入口から共通の上下限制限・単一 Undo・入力値の記憶へ接続した
    （[ADR 0177](decisions/0177-piano-batch-note-velocity.md)）。
    `piano-velocity-batch-ui`・`piano-velocity-batch-targets-ui`・`piano-velocity-batch-cancel-ui` で検査する。
    実画面・VoiceOver・聴感はユーザー確認待ちである。
  - 単一トラックの右ドラッグ枠選択と Command＋クリックの追加・解除、選択を保持する右メニューと一括削除を接続した。
    `piano-note-selection-ui`・`piano-note-selection-cancel-ui`・`piano-note-selection-delete-ui`・`piano-note-linked-delete-ui` で確認する。
    選択済みの本体ドラッグは選択全体の移動へ接続した（[ADR 0178](decisions/0178-piano-batch-note-movement.md)）。
    共通の時刻差・音程差・上下限制限、解放時の吸着と Shift、入力記憶の保持と単一 Undo を持つ。
    `piano-note-move-batch-ui`・`piano-note-move-batch-targets-ui`・`piano-note-move-batch-cancel-ui` で確認する。
    選択済み両端の一括伸縮も、反対端保持・共通の時刻差と最短長・入力記憶不変・単一 Undo へ接続した
    （[ADR 0179](decisions/0179-piano-batch-note-resizing.md)）。
    `piano-note-resize-batch-ui`・`piano-note-resize-batch-targets-ui`・`piano-note-resize-batch-cancel-ui` で確認する。
    矢印の選択全体の移動・移調・右端伸縮も、ネイティブ入力順と現行幾何の相対差分で接続した（[ADR 0181](decisions/0181-piano-reference-workflow-and-keys.md)）。
    `piano-key-step-ui`・`piano-key-step-bounds-ui` で履歴・リンク・共通制限と連続入力を確認する。
    Command 複製移動も接続した（[ADR 0184](decisions/0184-piano-note-duplicate-drag.md)）。
    `note-duplicate-model`・`piano-note-duplicate-ui`・`piano-note-duplicate-targets-ui`・`piano-note-duplicate-cancel-ui` は
    元と強弱の保持・複製側の選択・リンクの重複排除・解放時の吸着・取消・単一 Undo と保存を確認する。
    自動／固定グリッドの操作子と共有幅も接続した（[ADR 0185](decisions/0185-piano-auto-and-fixed-grid.md)）。
    `piano-auto-grid-ui`・`piano-fixed-grid-ui` と GPU 読戻しで、ズーム・吸着・入力長保持・取消・再生成を確認する。
    三連符・付点の固定幅も同じ経路へ接続した（[ADR 0186](decisions/0186-piano-triplet-and-dotted-grids.md)）。
    `piano-rhythmic-grid-ui` は実操作子・音価表示・入力と線・矢印とルーラー・入力長保持・取消・Undo / Redo・保存を確認する。
    拍・4 拍の目印は吸着幅と別に生成し、粗い幅・縮小時も保つ（[ADR 0187](decisions/0187-piano-independent-time-guides.md)）。
    `piano-time-guides-ui` は製品描画フレームの色・強線の優先・同位置の重複排除・吸着幅不変を確認する。
    選択と削除は [ADR 0176](decisions/0176-piano-note-selection-and-deletion.md)。実画面と VoiceOver はユーザー確認待ちである。
  - ドラム入力：音色名の行から親フォルダの MIDI へ入力し、通常のパターン複製ではノートの中身を独立させる。

### P2-08: 書出しとプロジェクト管理

WAV の形式・レート・余韻・ディザを選ぶ GUI と、プロジェクトごとの前回設定の保持を接続した。
記録済みの曲・参照する素材と状態を専用パッケージへ固定し、同梱 CLI の別プロセスで処理する（[ADR 0254](decisions/0254-gui-snapshot-export.md)）。
プロジェクトの保存レートは未定義のため、初回は出力のレート、出力未準備なら 48 kHz を使う。
処理済みの率と仕上げ中の表示を接続した（[ADR 0255](decisions/0255-export-progress.md)）。
途中取消も接続した（[ADR 0256](decisions/0256-export-cancellation.md)）。
前回設定をパッケージへ保持し、移動・コピー先へ引き継ぐ（[ADR 0257](decisions/0257-portable-export-settings.md)）。実画面・VoiceOver は UC-53〜56 の確認待ちである。
任意のサンプルピークノーマライズを CLI・GUI へ接続した（[ADR 0258](decisions/0258-peak-export-normalization.md)）。LUFS・トゥルーピークの処理ではなく、画面・VO・聴感は UC-57 に残す。
CLI の出力先も元パッケージの外へ限定し、復旧前に原本の上書きを拒否する（[ADR 0259](decisions/0259-export-destination-boundary.md)）。
GUI もスナップショット準備前に出力の親を解決・固定し、リンクの切替による元パッケージへの転送を防ぐ（[ADR 0260](decisions/0260-pinned-gui-export-destination.md)）。
完了後の明示的な Finder 表示を接続した（[ADR 0261](decisions/0261-reveal-export-output.md)）。実画面・VoiceOver は UC-58 の確認待ちである。
数値欄は入力中の文字列を保持し、不正な有効欄は理由を示して開始を拒否する（[ADR 0262](decisions/0262-export-numeric-drafts.md)）。
この Mac の共通既定設定の保存・適用・初期化を接続した。プロジェクトの前回値を優先し、共通値は別の Mac へ移さない（[ADR 0263](decisions/0263-common-export-defaults.md)）。
名前付き書出しプリセットの保存・適用・削除を接続した。この Mac の明示選択で、前回設定や共通既定値は変えない（[ADR 0264](decisions/0264-named-export-presets.md)）。

- 各種形式、ステム、LUFS 等のノーマライズ、SRC の選択、デバイスのプリセット、テンプレート、再リンク、収集して保存、履歴 UI（一覧・復帰・変更要約・タグ）、定期バックアップを接続する。
- 初期の書出しは曲全体のミックス 1 ファイル（ミュート・ソロ・関連 FX・末尾の余韻調整を反映、初回は WAV・24 bit・プロジェクトのサンプルレート、以後は前回の設定を保持）とし、CLI の別プロセスで処理する（[書き出し](interaction-design.md#書き出し)）。
- 受入: 再生との一致、形式・レート・チャンネル、失敗時の原本保持、処理中の編集との隔離、素材の hash、復旧・Undo を確認する。

### P2-09: Mac の操作と1曲の受入

- ジェスチャーの選択肢、触覚、ホットコーナー、初期ショートカットを編集・ミックスへ接続する。
- 機器なしの入力回帰を先に作り、最後にユーザーの実操作・聴感で「作者自身が 1 曲を Echode だけで作れる」（[操作要件](interaction-design.md) の一連の制作フロー）を確認した時点で Phase 2 の完了とする。
