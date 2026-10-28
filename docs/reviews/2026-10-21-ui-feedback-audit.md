# 2026-10-21 操作の要望の照合

ユーザーから届いた操作の問題と改善の要望 8 件を、操作要件・ADR・実装に照らして確認した。
確認はソースの読み取りと、機器と画面を開かない一時テスト 1 本（実行後に削除）で行った。
アプリの起動、実画面の操作、機器を開く検査はしていない。操作感と聴感はユーザー担当である。

仕様の正本（[操作要件](../interaction-design.md)・ADR）の更新は、末尾の未決事項への回答を受けてから行う。
この記録は発見時点の証拠であり、対応後も書き換えない。

## 一覧

| ID | 要望 | 仕様 | 実装 |
| :--- | :--- | :--- | :--- |
| UIF-01 | ペインのリサイズでアレンジのトラック幅が動く | 不具合 | 原因 2 つを確認 |
| UIF-02 | サンプルブラウザからの D&D | 未定義 | 未実装 |
| UIF-03 | クリップの D&D による移動 | 決定済み（P2-07） | 未実装 |
| UIF-04 | ダブルクリックで既定値、トリプルクリックで値入力 | 未定義 | 未実装 |
| UIF-05 | 試聴のクリック時の遅れ | ADR 0135 の既知の制約を含む | 原因候補 4 つ、未計測 |
| UIF-06 | 音声以外を表示しない | 未定義 | フォルダ表示は全ファイル |
| UIF-07 | 波形のプレビュー | 未定義 | 未実装 |
| UIF-08 | ピアノロールとデバイスを別々に開閉し、大きさを保って縦に積む | ADR 0131 の決定と異なる | 1 つの高さを分け合う |
| UIF-09 | クリックしてもフォーカスがブラウザへ移らない（2026-10-22 追加） | 不具合 | 原因の候補あり |
| UIF-10 | 試聴を連打すると素材がループして聞こえた（2026-10-22 追加） | 不具合の疑い | 再現なし、仮説あり |

## UIF-01: ペインのリサイズでトラック幅が動く

原因 A（確認）: アレンジとピアノロールの `CAMetalLayer` は、前のフレームを新しい枠へ引き伸ばす。

- 一時テストで `contentsGravity = resize`、`layerContentsPlacement = scaleAxesIndependently`、
  `layerContentsRedrawPolicy = never` を確認した。
- `drawableSize` は描画スレッドが次の run loop で変える（`CanvasRenderer.wake`）。
  枠は主スレッドで即座に変わるため、新しいフレームが届くまで古い絵が伸縮する。
- ヘッダ列も Metal で描くため、横のペインの境界ではヘッダ列とクリップが横に、エディタの境界では行が縦に伸び縮みして見える。

原因 B（確認）: `ArrangePaneView.layout` が `constrainedHeaderWidth` の結果を `headerWidth` へ書き戻す。

- 狭い幅や幅 0 の layout を 1 回通ると、ヘッダ幅が縮んだまま戻らない。
- 一時テストの値: 220 → 幅 0 を経由 → 120。220 → 幅 400 を経由 → 150 のまま。
  新しいビューで幅 0 の layout を先に通すと、既定の 160 ではなく 120 になる。
- 通常のアレンジ幅（520pt 以上）では上限 240 に達しない。
  SwiftUI がペインの切替や初回に幅 0 の layout を行うかは、実画面で未確認である。
- 既存の `ArrangeTests` は幅 750 の layout を 1 回見るだけで、幅の往復を検査していない。

## UIF-02: サンプルブラウザからの D&D

- ブラウザの行にドラッグ元（`onDrag` / `draggable`）がない。
- `ArrangeView` はドラッグの型を登録していない。`ArrangeHeaderView` は FX の型だけを受ける。
- 仕様も未定義である。[ADR 0117](../decisions/0117-workflow-interactions.md) と
  [ADR 0122](../decisions/0122-sample-library-search-and-placement.md) はダブルクリックの配置だけを決める。
  ドロップの記載は、ドラムラックへの配置（[ADR 0116](../decisions/0116-folder-drum-rack.md)）だけである。
- 配置そのものは既存の `audio.import`（トラックと tick を受ける）で足りる。
  ドロップ位置からトラックと tick を決める入力と、吸着・新規トラックの規則が欠けている。

## UIF-03: クリップの D&D による移動

- 要件は決定済みである（[ADR 0117](../decisions/0117-workflow-interactions.md)、
  [アレンジの選択と編集](../interaction-design.md#アレンジの選択と編集)）。P2-07 の範囲で未実装。
- アクション登録簿に移動がない。クリップ系は `clip.resize`・`clip.link`・`clip.set_gain`・`clip.set_fades`・
  `audio.place`・`audio.import` だけで、`clip.resize` も Swift から呼ばれていない（トリムの UI もない）。
- `ArrangeView.mouseDown` は、トラック・クリップの選択と、選択済み音声クリップのゲイン・フェードのハンドルだけを扱う。
- 現行モデルは同じトラックでの配置の重なりを禁じる（[ADR 0022](../decisions/0022-clip-content-and-placement.md)、
  [ADR 0071](../decisions/0071-clip-overlap-index.md)）。
  ADR 0117 の「重ねる」（初期値）と自動クロスフェードは P2-07 で不変条件ごと拡張する計画であり、移動先が重なる時の扱いを決める必要がある。

## UIF-04: ダブルクリックで既定値、トリプルクリックで値入力

現状、値を持つ操作子に既定値へのリセットと数値入力の起動はない。数値入力はインスペクターの `ControlNumberField` だけにある。

| 操作子 | 場所 | 単クリック | 既定値の情報 |
| :--- | :--- | :--- | :--- |
| トラックのフェーダー・パン | `ArrangeHeaderView` | ポインターの絶対位置へ跳ぶ | 0 dB・中央（自明） |
| デバイスのパラメーター | `WorkspaceParameterView`（`NSSlider`） | 位置へ跳ぶ | `DeviceParameter.defaultValue` を受信済み |
| クリップのゲイン・フェード | `ArrangeView` のハンドル | 相対ドラッグ | 0 dB・フェードなし |
| インスペクターの数値 | `MixInspectorView` | 入力欄 | ― |

- 既存のダブルクリックには値以外の意味がある。ピアノロールの空白はノート挿入（`PianoRollView`）、
  ブラウザは配置、FX 候補は追加、パレットは実行、仕様上のアレンジの空白はピアノロールを開く。
- macOS は 2 回目のクリックの時点で 3 回目が来るか分からない。
  2 回目で即座にリセットすると、3 回目で入力欄を開く前に値が一度既定値へ変わる。
- 単クリックで絶対位置へ跳ぶ操作子では、ダブル・トリプルクリックの各クリックが値の変更と Undo を作る。
- プラグインの値の文字入力に必要な CLAP の `text_to_value` は未接続である（`value_to_text` だけを使う）。

## UIF-05: 試聴のクリック時の遅れ

[ADR 0135](../decisions/0135-sample-audition.md) の経路は、選択の変更から次の順に進む。

1. 試聴スレッドがファイル全体を復号する（`decode_audio_file`）。
2. 出力が閉じていれば専用の IOProc を開く。最後の音から 5 秒で閉じる。
3. 素材と機器のレートが違えば、全体を rubato の FFT で SRC する（`device_planes`）。
4. 次のオーディオブロックの先頭で鳴らす。

- 1 と 3 はファイル長に比例し、2 は 5 秒以上操作しなかった後の最初の選択で入る。
  44.1 kHz の素材を 48 kHz の機器で鳴らす場合は、毎回 3 が入る。
- ADR 0135 は「全体を読み終えるまで鳴らない」を既知の制約として残している。
- 仮説（未確認）: 一覧の行の `onTapGesture(count: 2)` が、単クリックの選択の確定をダブルクリックの判定まで遅らせる可能性がある。
- 診断の不足: スナップショットの `audition` は段階を持つが時刻を持たず、どの段で待ったかを区別できない。

## UIF-06: 音声以外を表示しない

- フォルダの一覧は、フォルダ・`.echode`・その他の全ファイルを表示する。登録フォルダ全体の検索は音声だけを返す。
- ブラウザは「開く」ボタンでプロジェクトを開く役割も持つ（空の時の案内も「プロジェクトを探す」）。
  プロジェクトを隠すかどうかの判断が要る。
- 音声と判定する拡張子は wav・aiff・aif・caf・mp3・m4a・aac・flac。復号は AudioToolbox なので、これらは読める見込みである。

## UIF-07: 波形のプレビュー

- 未実装で、仕様もない。
- 既存のピーク（`echode-history` の `peaks.rs`、[ADR 0058](../decisions/0058-waveform-peaks.md)、
  [ADR 0110](../decisions/0110-apple-silicon-draw-and-peaks.md)）はパッケージ内の素材向けで、取り込む前のファイルには使っていない。

## UIF-08: ピアノロールとデバイスビューの開閉と積み方

- 下部は高さ 1 つ（`editorHeight`）のペインで、中のセグメント（ピアノロール／デバイス／両方）で切り替える。
  これは [ADR 0131](../decisions/0131-device-view-and-responsive-parameters.md) の決定である。
- 「両方」は同じ高さを 2 つで分け合うため、開くとそれぞれが縮む。
- ツールバーの「Toggle Note Editor」（⌥⌘3）は下部全体を開閉し、「Devices」は表示するだけで閉じない。セグメントの選択は保存しない。
- 要望は ADR 0131 の下部の切替を置き換える。
  ウィンドウの最小の高さ（580pt）と、アレンジの最小の高さ（225pt）との関係を決める必要がある。

## 2026-10-22 の回答と追加の所見

決まった操作は [操作要件](../interaction-design.md) へ移した。
UIF-02・06・07 は [共通サンプルブラウザ](../interaction-design.md#共通サンプルブラウザ)、
UIF-03 は [アレンジの選択と編集](../interaction-design.md#アレンジの選択と編集)、
UIF-04 は [値の操作子の既定値と数値入力](../interaction-design.md#値の操作子の既定値と数値入力)、
UIF-08 は [デバイスビューと FX 一覧の役割](../interaction-design.md#デバイスビューと-fx-一覧の役割) にある。
ADR は各項目を実装する時に書く。UIF-08 は ADR 0131 の下部の切替を置き換える。

### UIF-01

ユーザーの症状は「ドラッグ中だけ伸縮し、離すと戻る」で、原因 A に当たる。
原因 B は実画面では未確認だが、幅の往復で再現するため同じ作業で直す。

### UIF-05: 遅れはクリックの経路にある見込み

ユーザーの観察は次のとおり。

- クリックでの試聴は、ファイルの長さに関わらず一定の遅れがある。
- 上下矢印キーで選択を移した時の試聴は遅れなかった。

矢印キーも同じ `onChange(of: browser.selection)` から試聴スレッドへ進む。
このため、上記の段 1〜3（復号・出力を開く・SRC）は、ユーザーの素材では主な原因でないと見る。
クリックだけが通る経路は、一覧の各行に付けた `onTapGesture(count: 2)` である。
macOS の SwiftUI では、行にダブルタップの認識を付けると単クリックの選択が抑えられる・遅れるという報告がある。
`contextMenu(forSelectionType:menu:primaryAction:)` の `primaryAction` でダブルクリックを受ける方法が代わりに挙がっている。
実画面での計測は未実施であり、遅れの量と原因は修正時に時刻付きの診断で確かめる。

### UIF-09（追加）: クリックしてもフォーカスがブラウザへ移らない

- ユーザーの報告: 一覧をクリックしてもキーボードのフォーカスがアレンジに残り、矢印キーで試聴を確かめられなくなった。
- `makeFirstResponder` を呼ぶのは、アレンジ・ヘッダ・ピアノロール・分割線の `mouseDown` だけである。ブラウザを押してもアレンジから明示的に奪ってはいない。
- 行のタップの認識がクリックを受けるため、一覧そのものが最初のレスポンダーにならない可能性が高い。UIF-05 と同じ原因の候補である。

### UIF-10（追加）: 試聴を連打すると素材がループして聞こえた

- ユーザーの報告で、再現できていない。
- 試聴のボイス（`echode-engine` の `audition.rs`）は 1 回鳴らすと終わり、繰り返す経路を持たない。
- 仮説: 連打の 2 回が行のダブルクリックと判定され、`open(file)` から `audio.import` で選択トラックの再生ヘッド位置へ素材が配置された。
  ループ再生中なら、配置したクリップが毎周鳴る。アレンジに意図しないクリップや Undo の項目が増えていれば、この仮説に当たる。

### 他の DAW の試聴

公開マニュアル・フォーラムで確かめた範囲である。内部の読込み方式は公開されていない。

- **Ableton Live**: プレビューのトグルを入れると、選択または上下矢印で鳴る。トグルなしでも右矢印か Shift＋Enter で鳴らせる。
  プレビュー欄に波形があり、クリックした位置から鳴らせる。
  Raw がオフなら再生中は次の小節頭から曲に合わせて鳴らし、Raw がオンなら元のテンポで 1 回鳴らす。
  音量はメインの Preview/Cue つまみで、Cue 出力を別のヘッドホンへ分けられる。
- **Bitwig Studio**: 下部のプレーヤーに再生ボタンと音量フェーダーがある。選択時の自動試聴は切り替えられ、オフなら右矢印で鳴らす。
  元の速さか、曲のテンポに合わせるかを選べる。ブラウザはプラグインのプリセット（FXP・VSTPRESET・CLAP の各社形式）、MIDI、プロジェクトも扱う。
- **REAPER**: Media Explorer に選択時の自動再生があり、テンポ合わせ（1 倍・1/2・2 倍）と小節頭からの開始を選べる。選択中の素材の波形を表示できる。

出典:
[Ableton Live マニュアル](https://www.ableton.com/en/manual/working-with-the-browser/)、
[Bitwig ユーザーガイド: 共通要素](https://bitwig.com/userguide/latest/common_browser_elements)、
[Bitwig ユーザーガイド: ブラウザ](https://bitwig.com/userguide/latest/browsers)、
[Ableton フォーラム: Cue とプレビュー音量](https://forum.ableton.com/viewtopic.php?p=1290329)、
[REAPER の更新案内](https://dlz.reaper.fm/userguide/REAPER%20main%20changes%20665%20to%20668.pdf)、
[Apple Developer Forums: List の行のダブルクリック](https://developer.apple.com/forums/thread/654154)、
[Apple Developer Forums: Table の行のダブルクリックと選択](https://developer.apple.com/forums/thread/707421)、
[zenn: macOS の List のダブルクリック](https://zenn.dev/mtgto/articles/swiftui-macos-double-click-list)。

### 同日の追加回答

- UIF-05: 試聴を直す順番（クリックの経路と計測、先頭から読みながら鳴らす、出力を開いたままにする、音量などの操作）を決め、
  [共通サンプルブラウザ](../interaction-design.md#共通サンプルブラウザ) へ移した。
- UIF-06: MIDI は今から表示し、読込みを接続するまで読み込めないと分かる表示にする。
  プラグインのプリセットは [P2-05](../phase2.md#p2-05-vst3--au-とブラウザの対応形式) で FX 候補パネルの側に置く。
- UIF-08: アレンジを隠す状態は、ツールバーの切替ボタンとショートカットで入る。
- 「重ねる／置き換える」は、現行の操作要件どおりアレンジとピアノロールで別々に持つ。

### UIF-10 の続き: 配置の仮説は外れた

ユーザーの追加の観察は次のとおり。

- 曲は停止中だった。ダブルクリックはしておらず、色々なサンプルのクリックと上下キーの操作中に起きた。
- 同じサンプル全体が頭から最後まで鳴り、また頭から鳴った。しばらくして自然に止まった。

停止中は配置したクリップを鳴らさないため、配置の仮説は当たらない。
試聴の要求を送るのは、ブラウザの選択の変更（`onChange(of: browser.selection)`）だけである。
試聴スレッドは溜まった要求を最新 1 件にまとめ、ボイスは 1 回で終わる。
このため、同じパスへの要求が、素材の長さ以上の間隔を空けて何度も届いたと見る。選択が同じファイルへ何度も戻ったことになる。

候補は次の 2 つで、どちらも未確認である。

- 行の `onTapGesture(count: 2)` が単クリックの選択を後回しにし、後から届いた選択の変更が上下キーの選択と入れ替わった（UIF-05・09 と同じ経路）。
- `preservedSelection` は、選択が一覧になければ先頭の項目を選ぶ。登録フォルダの再走査や検索語の変更で、利用者の操作なしに選択が変わり試聴が鳴る。

再発時に区別できるよう、修正では試聴の要求の履歴（時刻・要求番号・パス・操作の種類）を診断に残す。
利用者の操作でない選択の変化（再走査・検索による付け替え）で試聴を鳴らすかどうかは、ADR 0135 の見直しとして修正時に決める。

## 修正の進め方

操作の正本は [操作要件](../interaction-design.md) である。ここには、修正の順番・場所・回帰・受入を置く。
各単位は AGENTS.md と [開発手順](../development.md) に従い、製品の経路で再現してから直し、同じ作業で ADR・回帰・診断を更新する。
実画面の操作感・聴感はユーザー担当とし、自動検査の合格と区別して報告する。
Swift の指摘は `make known-bugs`（Rust の `audit_` テスト）の対象外である。各単位の回帰を修正と同時に足す。

推奨する順番は次のとおり。1 と 2 は小さく独立しており、3 以降の前提になる。

### 1. ブラウザのクリック経路と試聴の診断（UIF-05 の①・09・10）

- 場所: `WorkspaceFileBrowser.swift` の行の `onTapGesture(count: 2)`。選択を遅らせない受け方（`contextMenu(forSelectionType:menu:primaryAction:)` など）へ置き換える。
  診断は `crates/echode-audio-io/src/audition.rs` の `AuditionReport`。
- 診断: 要求ごとに受付・復号の完了・出力の準備・発音の時刻を持たせ、直近の要求の履歴（番号・パス・操作の種類）を残す。
  `make debug-report` で読めるようにする。
- 決めること: 再走査・検索による選択の付け替え（`preservedSelection`）で試聴を鳴らすか。ADR 0135 の改訂として記録する。
- 回帰: 単クリックで配置しない、ダブルクリック・Return で配置する、選択の変更 1 回で要求 1 回。
  controller の `sendAudition` の差し替えで確認する。必須契約 `sample-audition-ui` を見直す。
- 受入: 自動回帰の合格と、診断の時刻で遅れの段を示すこと。クリックの体感・フォーカスの移動はユーザー確認。

### 2. ペインのリサイズ（UIF-01）

- 場所: `ArrangeView` と `PianoRollView` の Metal レイヤー。リサイズ中に古いフレームを引き伸ばさない配置にし、`drawableSize` と枠の更新を揃える。
  `ArrangePaneView.layout` は利用者の指定したヘッダ幅を保持し、表示の時だけ制限する。
- 回帰: 幅 0・狭い幅を往復してもヘッダ幅が戻る（`ArrangeTests`）。レイヤーの配置設定を検査する。
- 受入: 自動回帰の合格。ドラッグ中の見え方はユーザー確認。

### 3. ブラウザの表示対象と D&D（UIF-06・02）

- 場所: `FileBrowserState.swift` の `BrowserFile` と `load`（表示する種類）、`SampleLibraryScan`。
  ドラッグ元は `WorkspaceFileBrowser.swift`、ドロップ先は `ArrangeView`（ファイル URL の型を登録する）。
- 配置: ドロップ位置の行とカメラからトラックと tick を決め、グリッドに吸着し、Shift で解除する。
  既存の `audio.import` を使い、最終トラックより下では `track.add` と同じトランザクション（`actions.transaction`）で 1 回の Undo にする。
  音声以外と、読込みのない MIDI のドロップは受け付けない。
- 回帰: ドロップの位置・吸着・Shift・新規トラック・Undo / Redo・保存の再読込（`ArrangeTests`、`SampleBrowserTests`）。
  Finder と同じ型で受けること。表示の判定は `SampleBrowserTests`。

### 4. 試聴の先読み・波形・出力の常駐・操作（UIF-05 の②〜④・07）

- 場所: 復号は `echode-history` の `decode_audio_file`、試聴は `echode-audio-io` と `echode-engine` の `audition.rs`、SRC は `resample.rs`。
- 設計: 制御側が塊ごとに復号・SRC して受け渡し、音声側は確保なしに塊をたどる。同じ読込みからピークを作り、ブラウザの波形に使う。
  ブラウザの表示中は出力を閉じない。音量は補間して変え、開始位置はサンプル単位で決める。
- 回帰: `block_size.rs` に塊の境界・開始位置・音量を含む試聴を足す。RTSan、機器なしの出力での長い素材の開始時間、Undo の対象外。
- 文書: ADR 0135 を置き換える ADR。

### 5. 下部の開閉と積み方（UIF-08）

- 場所: `WorkspaceState.swift`（`BottomView` を、ピアノロール・デバイス・エディタ最大化の 3 つの開閉へ）、`WorkspaceView.swift`、`WorkspaceCommand.swift`。
- 決めること: デバイスビューの固定の高さ、最大化のショートカット。⌥⌘3 は現在の下部の開閉で、⌥⌘4 は空いている。
  保存済みの `workspace.layout.v1` からの移行。
- 回帰: `WorkspaceTests` の寸法（両方開いた時、収まらない時にピアノロールだけが縮む、最大化、最小ウィンドウ）。
- 文書: ADR 0131 の下部の切替を置き換える ADR。

### 6. 値の操作子（UIF-04）

- 場所: `ArrangeHeaderView.swift`（フェーダー・パン）、`WorkspaceParameterView.swift`（`ParameterSlider`）、`ArrangeView.swift`（クリップのハンドル）。
- 相対ドラッグ、クリック数による既定値・入力欄、3 回目での取り消しを 1 つの共通の部品にまとめる。
- プラグインの文字入力は、CLAP の `text_to_value` をヘルパー（`threads.rs`）・通信（`crates/echode-sandbox/src/params.rs`）・FFI（`device.rs`）へ足す。
- 回帰: 合成したマウスイベントで、単クリックで値が変わらない、ダブルクリックで既定値と Undo 1 回、トリプルクリックで元の値と入力欄、Escape で変更なし。
  `MixControlTests`、`DeviceParameterTests`。

### 7. クリップの移動と複製移動（UIF-03）

- 場所: `crates/echode-actions/src/action.rs` に移動（トラックと開始位置）と複製移動のアクションを足す。
  置き換えは、移動先のトラックで重なる配置の区間を削る。`ArrangeView` のドラッグ（吸着・Shift・Command）。
- 回帰: `crates/echode-history/tests/properties.rs` の操作の生成、`Project::check_invariants`、Undo / Redo、保存の再読込、再生と CLI 書出しの一致。
- 文書: ADR。P2-07 のアレンジ編集の先行分として [Phase 2 の進め方](../phase2.md) の進捗に記す。

## 検査

- 一時テスト（`ArrangePaneView` の幅の往復と Metal レイヤーの設定の読み取り）を `swift test -q --filter` で 1 回実行し、ファイルを削除した。
- 製品のソースは変更していない。回帰テストの追加は、各項目の修正と同じ作業で行う。

## 対応（2026-10-22）

上の所見は発見時点の記録として残す。対応した単位と未確認の点をここに足す。

### 1. ブラウザのクリック経路と試聴の診断

- [ADR 0143](../decisions/0143-browser-click-and-audition-timing.md) で接続した。一覧を AppKit の表に置き換え、押した時点で選択・フォーカス移動・試聴の要求を行う。
  2 回目のクリックと Return だけが配置する。
- 決めたこと: 再走査・検索などブラウザ自身の選択の付け替えでは、試聴を始めも止めもしない。
- 再現: SwiftUI の一覧をテスト内のウィンドウに載せ、合成したクリックを送る一時テストを 2 通り（行のダブルタップの認識あり・なし）実行した。
  どちらも選択が変わらず、テストのプロセスでは SwiftUI の一覧のクリック経路を再現できなかった。一時テストは削除した。
  遅れはユーザーの実画面の観察に基づき、回帰は置き換えた表の実際の入力で持つ。
- 回帰: 必須契約 `sample-browser-click-ui`（負の対照あり）、`echode-audio-io` の段の時刻と履歴、FFI・ブリッジ・`AuditionLog` の単体回帰。
- 診断: スナップショットの `audition.history` と、アプリが書く `~/Library/Logs/Echode/audition.ndjson`（`make debug-report` の `app-logs/`）。
- 未確認: 実画面のクリックの体感とフォーカス、表の見た目、遅れの量、UIF-10 の原因。ユーザーの確認と再発時の記録を待つ。
  プラグイン候補とコマンドパレットの一覧には同じ行のダブルタップの認識が残る。

### 2. ペインのリサイズ

- [ADR 0144](../decisions/0144-unscaled-canvas-resizing.md) で接続した。
  両方の製品表示面を左上固定にし、枠・bounds の変更時に主スレッドが drawable のサイズを更新する。
  入力と違うサイズの drawable はクリアして再試行し、GPU 完了を主スレッドで待たない。
- 指定ヘッダ幅は layout で変更せず、現在のペインの制限値だけを入力・描画へ渡す。文書内の表示記憶にも指定幅を残す。
- 再現と回帰: `CanvasResizeTests` の両契約を修正前の製品 View で失敗させた。
  必須契約 `canvas-resize-ui`・`header-width-ui` に負の対照を持たせ、倍率 1・2 と端数・古い drawable の拒否も単体回帰へ追加した。
- 診断: `ArrangeMeasurement.sizeMismatches` でサイズ不一致の再試行を数え、[デバッグ手順](../debugging.md)へ切分けを追加した。
- 未確認: 実画面でのペイン境界のドラッグ、倍率の違う画面への移動、VoiceOver の操作感。ユーザー確認とする。

### 3. ブラウザの表示対象と D&D

- [ADR 0145](../decisions/0145-sample-browser-file-drops.md) で接続した。
  フォルダ一覧と登録検索は制作向けの種類を返し、MIDI の未対応を名前へ表示する。
  ブラウザと Finder 共通のファイル URL をアレンジで受け、現在の表示行・ポインター位置へ配置する。
- 拍グリッド・Shift 解除・最終行より下での最上位末尾のトラック追加を持つ。
  新規トラックと取込みはトランザクション 1 回にし、失敗時は巻き戻して Redo を保持する。
- 再現と回帰: 修正前の表示・検索と、実 WAV のトランザクション内の拒否を失敗させた。
  `SampleBrowserContentTests`・`SampleAudioDropTests`・`audio_import_transaction` に製品経路の回帰と必須契約の負の対照を持つ。
- 診断: 既存の契約ログと文書エラーを使い、型の拒否・座標・文書交換・復号・配置重複をデバッグ手順へ追加した。
- 未確認: 実際のドラッグ、カーソルと一覧の見え方、フォルダ権限、VoiceOver はユーザー確認とする。
  複数ファイル同時配置とドラッグ中の自動スクロール、配置予定の波形は未接続である。

### 5. 下部の開閉と積み方

- [ADR 0146](../decisions/0146-independent-editor-stack.md) で個別開閉・固定高のデバイス・アレンジ非表示を接続した。
  修正順の 4 番目（試聴の先読み等）は未着手のまま、独立したこの単位を先に進めた。
- 再現: 共通コマンドを通し、デバイスを開くとピアノロールが隠れることと、もう一度押しても閉じないことを修正前に失敗させた。
- 回帰: `WorkspaceLayoutTests` に全開閉状態の寸法・製品の中央列の枠と順序・最大化・希望高の保持・旧設定の移行を追加した。
  必須契約 `workspace-layout-ui`・`workspace-stack-ui` は負の対照を持つ。プラグイン側の新しいキーも既存の回帰へ追加した。
- 診断: 設定の v1／v2 と最大化・開閉・希望高を区別する手順を追加した。ログ形式と音声経路は変更していない。
- 未確認: 実画面の最小高の変化・開閉・境界のドラッグ、キー・メニューの使い心地と VoiceOver はユーザー確認とする。

## 対応（2026-10-24）

### 4. 試聴の先読みと波形

- [ADR 0151](../decisions/0151-streaming-sample-audition.md) で、有限の先読み・連続 SRC と同じ読込みからの部分波形を接続した。
  出力の常駐と試聴の音量・開始位置等は残る。
- 機器なしの製品経路で全復号を待つことを先に失敗させ、全復号前の発音と部分波形・読込み中の停止を回帰へ追加した。
  元の失敗の証拠は `target/audition-stream-repro.log` に保持する。
- 必須契約 `sample-audition-stream`・`sample-preview-ui`、SRC の全体変換との比較、ブロック長比較と RTSan の対象を追加した。
  読込み中の出力をアイドル扱いで閉じる経路も再現し、`sample-audition-loading` で保持と停止後の閉鎖を確認する。
  開始時間は通常の合否へ混ぜず `make spike-audition` で別途記録する。
- 実機の待ち時間・聴感、ブラウザの波形と読込み状態の見え方・VoiceOver はユーザー確認とする。

### 4. 試聴出力の常駐と操作の続き

- [ADR 0152](../decisions/0152-browser-audition-controls.md) で、ブラウザ表示中の無音出力の保持・
  試聴音量の 5 ms 補間・自動試聴の切替・右矢印と再生／停止ボタン・波形クリックの元サンプル位置からの試聴を接続した。
- 先に期限を過ぎた時刻で出力が閉じる失敗を再現し、`target/audition-resident-repro.log` に保持した。
  実機の開始時間の計測と区別する。
- `sample-audition-resident`・`sample-audition-position`・`sample-audition-gain`・`sample-audition-controls-ui` と、
  ブロック長・RTSan・C ABI / Swift の往復へ回帰を追加した。診断は設定・無音の準備失敗・元サンプル位置を区別する。
- 実画面の見え方・VoiceOver、機器切替・曲との加算・聴感はユーザー確認とする。

### 6. 値の操作子

- [ADR 0153](../decisions/0153-value-control-clicks-and-typed-entry.md) で、ヘッダの音量・パン、クリップのゲインとフェード、パラメーター欄に共通のクリック規則を接続した。
  単クリックは値を変えず、ドラッグは相対量、ダブルクリックは既定値を仮に表示して間隔後に 1 回記録し、3 回目は元の値へ戻して入力欄を開く。
- プラグインの値は CLAP の `text_to_value` をヘルパー・通信・FFI へ足し、表示単位で入力する。変換を持たないプラグインは数値で入力する。
- 再現: 修正前の必須契約 `track-mix-ui` はパンの単クリックで値が変わることを合格条件にしており、新しい規則でその 3 か所が失敗した。
  NSSlider の押した位置への跳びは所見どおりソースで確認した。
- 回帰: `mix-clicks-ui`・`clip-clicks-ui`・`fx-params-clicks-ui`・`plugin-text-parse` を負の対照付きで追加し、`track-mix-ui` を相対ドラッグへ改めた。
- 決めたこと: 入力欄の外へフォーカスが移った時は使える値を確定し、使えない値は破棄する。範囲外の入力は丸めずに拒否する。
- 未確認: 実画面のクリック間隔・欄の見た目・VoiceOver、入力中の Space が再生へ漏れないこと、第三者プラグインの文字変換はユーザー確認とする。

## 対応（2026-10-25）

### 7. クリップの移動と複製移動

- [ADR 0154](../decisions/0154-clip-move-and-duplicate.md) で、`clip.move`・`clip.duplicate` とアレンジのクリップ本体のドラッグを接続した。
  別トラックへの移動、四分音符への吸着と Shift の解除、離した時の Command による複製、単一 Undo を持つ。
- 移動先で重なる配置は、重ねるモードを接続するまで置き換える。内側の配置は消し、端をまたぐ配置は切り詰め、窓全体を含む配置は分割する。
  切り口には既定の 5 ms のフェードを付け、残した部分は `fixed` にする。
- 修正前の状態は所見どおりで、登録簿に移動がなく、本体の押下はクリップを選ぶだけだった。
- 回帰: モデルの単体テスト、履歴の `clip_move`（ジャーナル再生・保存）、CLI の `clip_move`（直接配置との書出し一致）、
  `ClipMoveTests`（実 View の合成イベント）、`properties.rs` の操作の生成を追加した。
  必須契約 `clip-move-ui`・`clip-move-model` を負の対照付きで登録した。
- 未接続: 重ねるモードと自動クロスフェード、⌘D の直後への複製、複数クリップ、端での自動スクロール、VoiceOver からの移動、トリム。
- 未確認: 実画面のドラッグの操作感・影の見え方・VoiceOver はユーザー確認とする。

## 追加対応（2026-10-28）

- 当時未接続だった単一クリップの ⌘D だけを、現在の窓の直後への独立した複製・単一 Undo・保存へ接続した
  （[ADR 0188](../decisions/0188-arrange-adjacent-clip-duplication.md)）。
  `ClipDuplicationTests`・`ClipDuplicationBoundaryTests` と負の対照付きの製品契約を追加した。
  複数クリップ・重ねるモード・トリム等の残りと、実画面のユーザー確認は引き継ぐ。
