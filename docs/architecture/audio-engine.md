# オーディオエンジン・ルーティング・音声解析アーキテクチャ（§7、§8、§11）

- 全体索引: [../ARCHITECTURE.md](../ARCHITECTURE.md)
- 操作要件の正本: [../interaction-design.md](../interaction-design.md)
- 現在の実装範囲: [../status.md](../status.md)

## 7. オーディオエンジン

### 7.1 スレッド

| スレッド | 優先度 | 役割 |
|---|---|---|
| I/O スレッド | リアルタイム（CoreAudio が用意する） | 1 ブロックの処理を始め、自分も処理に加わる |
| ワーカー（Workgroup の推奨本数から IO スレッドを引く） | リアルタイム（時間制約＋デバイスの Audio Workgroup） | グラフのノードを並列に処理する |
| 先読みワーカー | 高 | 先読み対象のトラックを数百 ms 先まで処理する |
| ディスク | 通常 | 音声ファイルの先読み |
| 制御 | 通常 | グラフの組み立て、プラグイン状態の取得、不要メモリの解放 |

実装範囲（2026-10-17）: 上表の並列 CPU グラフは S2 のスパイクであり、製品の内蔵 DSP・配置音声・フェーダーは I/O 側で順に処理する。
製品の独立した CLAP トラックは同時に投入し、ヘルパーの Workgroup は選択デバイスへ接続する
（[最適化監査 M27-01 / 05](../reviews/2026-10-17-macos27-optimization-audit.md)）。

コールバックはログを書かない。取りこぼしたステージと締め切りを越えたブロックを固定長の記録でリングへ入れ、制御スレッドが `PlaybackControl::service` でノード名を付けて書く。
自動ミュートは公開済みの状態の変化を制御スレッドが書く（[ADR 0322](../decisions/0322-product-log.md)）。

Phase 0 の固定グラフ・待機処理・機器計測は、非既定の `echode-engine/spike` feature へ分離する。
製品の機器問い合わせは feature なしで使い、計測と共有する読取りだけを `device.rs` に残す
（[ADR 0213](../decisions/0213-opt-in-engine-spike.md)）。

### 7.2 グラフ

- 再生計画と、その計画に対応するプラグイン音声バッファを 1 つの更新としてロックフリーキューで渡す。
  音声側が排他所有し、古い計画・バッファは制御側へ返して解放する。容量変更は準備と採用を分ける（[ADR 0102](../decisions/0102-exclusive-audio-graph.md)）。
- トランスポート・試聴のリングは音声側の専用命令型を運ぶ。公開のループ操作は計画を準備し、単一フェーダーは制御側で検証済みの集合へ変換する。
  未使用の別系統の受渡しを削除し、届かない命令を型から除く（[ADR 0253](../decisions/0253-product-only-playback-commands.md)）。
- 開いた出力の置換ヘルパー・共有領域は準備スレッドで作り、文書ワーカーは編集とトランスポートを続ける。
  制御用カタログの受渡し・後続編集の集約・既存状態の保持は [ADR 0142](../decisions/0142-background-plugin-graph-preparation.md) を参照する。
  出力の導入時は差分を送る前に準備スレッドを有効にし、再生はその差分の計画の公開まで始めない（[ADR 0333](../decisions/0333-install-diff-graph-off-worker.md)）。
- 製品の出力の置換で `MediaInputs`（テンポと音声配置）が前回の読込みと異なる時は、再生中も停止中も素材の読込み・SRC を `echode-media-prepare` で行う。
  読込み中の置換とループ変更は集約し、その間は前の計画が鳴る（[ADR 0147](../decisions/0147-output-prewarm-and-background-media.md)、[ADR 0330](../decisions/0330-playing-edit-media-and-tempo-retime.md)）。
- 位置は、鳴っている計画のテンポで解釈する。テンポの変わった計画は採用時にクロックの tick を保ち（`Update.retime`）、
  Session の位置移動は `LocateTick`・`RewindTick` でコールバックが変換する。サンプルの `Locate`・`Rewind` はオフラインと検査のために残す（ADR 0330）。
- `PlaybackControl::set_cached_tracks` で停止中に準備した平面を次の計画へ載せ、トラックのデバイス処理の代わりに読む。
  供給側の失効・レンダリング本体・再生中の切替は Phase 3（§7.8、[ADR 0108](../decisions/0108-phase1-loop-readiness.md)）
- スケジューラはノードごとの依存カウンタで動き、残りのノードがどれも実行中のときだけ待つ
  （[ADR 0003](../decisions/0003-audio-thread-does-not-wait.md)）
- 製品のコールバックは、ブロックのイベントをトラックごとに 1 回で振り分け、プラグインのトラックはキーの索引で引く。
  トラックごとに全トラック・全イベントを走査しない（[ADR 0236](../decisions/0236-callback-work-per-track.md)）
- イベント容量は制御側でブロック内の端の最大と同時発音数から求め、ループの周回と旧計画のノートオフも含める。
  ヘルパーの共有入力は 2 のべき乗で増やし、同じヘルパーの寿命では縮めない（[ADR 0240](../decisions/0240-block-local-event-capacity.md)）。
- 再生 ID の表は成功した計画のノートとトラックだけを保持する。残る ID の番号は保ち、除去した番号は再利用しない。
  古い表の容量は制御側で解放する。Undo とジャーナルのメモリ予算は別の未対応範囲である（[ADR 0267](../decisions/0267-current-playback-id-mappings.md)）。

### 7.3 レイテンシ補正（PDC）

全ノードのレイテンシから経路ごとの遅れを計算し、合流点（トラックの加算、コンテナの出口、センド、レシーバー、マスター）に遅延を入れてそろえる。
録音モニタリング用に低レイテンシモードを用意する。
Phase 2 のクリップ FX も経路に含め、バイパス・オフライン化やクリップの切り替わりでも時刻が跳ばないようにする。

### 7.4 先読み処理

ライブ入力に関係しないトラックは再生位置より数百 ms 先まで先行処理し、リングバッファから出力する。
つまみ操作やプラグイン GUI 表示時は反応遅れを避けるため自動でリアルタイム処理に戻す。

### 7.5 クリップの再生

- **Phase 1 の製品接続**: 素材は制御側で全体を読み、左右を個別に `rubato` でデバイスレートへ変換する。
  ステレオは CLAP メインポート間と書き出しまで維持する。配置音声のループ両端は最大 5 ms、フェーダー変更は 5 ms で補間し、マスター加算は `f64` とする
  （[ADR 0095](../decisions/0095-product-audio-buses-and-continuity.md)）。
  ループ先頭も計画採用前に全体準備に含め、`media_loader` が最後に成功した PCM を保持する
  （[ADR 0108](../decisions/0108-phase1-loop-readiness.md)）。
  同じハッシュ・メタデータ・デバイスレートの変換と、同じ内容位置・長さの未スケールの窓を共有する。
  配置ゲインとモノのステレオ昇格は、ブロックを読む時の係数とし、PCM を複製しない。
  ゲイン変更時の桁あふれは、準備時に保持した窓のピークから判定する
  （[ADR 0241](../decisions/0241-unscaled-placement-pcm.md)）。
  素材の異なる窓の和が素材以上なら、素材の面を 1 つだけ持ち、各配置は `ClipWindow` の範囲を読む（面の外は無音）。
  和が素材未満なら窓だけを複写し、最後の配置で素材の全体を手放す（[ADR 0273](../decisions/0273-shared-source-clip-windows.md)）。
- **サンプルレート変換**: 既定は `rubato`（比率一定は FFT、可変は sinc）。
  `r8brain-free-src`、`soxr`、`zita-resampler`、`libsamplerate`、`AudioConverter` は測って選択肢に加える
  （[ADR 0014](../decisions/0014-existing-implementations-as-options.md)）
- **MIDI**: 制御スレッドがノートをサンプル位置に落とし、オーディオスレッドはカーソルを進めるだけにする
  （[ADR 0035](../decisions/0035-playback-plan.md)、[ADR 0037](../decisions/0037-clap-events.md)、[ADR 0010](../decisions/0010-playback-samples.md)）。
  開始・位置移動で途中のノートを追いかけて鳴らす時、終端ちょうどのノートは鳴らさずノートオフだけを出す
  （[ADR 0164](../decisions/0164-note-end-chase.md)）
  折り返しと位置移動は鳴っている集合だけを止める。ノートオフで集合から O(1) で除き、終わったノートの履歴を走査しない。
  差替えは制御側で作ったトラック ID・ノート ID と開始位置の索引で照合し、発音時のキーを引き継ぐ
  （[ADR 0239](../decisions/0239-live-note-cursor-work.md)）。
  トラックとクリップの内容が同じ `Arc` で、テンポとレートも同じなら、そのトラックの計画・カーソルの索引・必要量を前回から使う。
  番号表は使用数を持ち、作り直したトラックの分だけ更新する（[ADR 0274](../decisions/0274-reused-track-plans.md)）。
  テンポ・レートだけが変わったトラックは、ノートの番号を前回の計画から訪れた順の位置で取り、計画に入るノートが同じなら使用数を更新しない（[ADR 0335](../decisions/0335-retimed-plans-keep-note-numbers.md)）。
  採用の `Cursor::take_over` は、索引を共有する変わらないトラックのカーソルの状態を入れ替え、探索・解放・追いかけ直しをしない。
  保持中のヘルパーが持つ声の印もカーソルが付け、新しいヘルパーだけへ追いかける（[ADR 0279](../decisions/0279-unchanged-track-voice-handover.md)）。
  制御側は渡す直前に、新しいカーソルを最後に公開された位置と追いかけの規則で置き、音声側はその後のノートだけを進める。
  作り直したトラックの声は、前の集合とノート ID で照合し、ループの前から続くノートを追いかけない規則も引き継ぐ（[ADR 0280](../decisions/0280-placed-cursor-and-voice-matching.md)）。
- **クリップ FX（Phase 2）**: 配置ごとのチェインに通してからトラックへ合流させる。終端では素材の入力とノートを止め、テールの範囲は処理を続ける
- **ループの継ぎ目とフェーダー**: 継ぎ目の前後に 5 ms のフェードをかける（[ADR 0011](../decisions/0011-preview-cache.md)）。
  フェーダーはキャッシュの後ろに置き、モノは等電力パン、ステレオはバランスパンとする。
  ミュートはソロより優先し、ピークはフェーダーとパン後の最大チャンネル値とする
  （[ADR 0039](../decisions/0039-bus.md)、[ADR 0012](../decisions/0012-fader-after-cache.md)）
- **一括のミキサー試聴**: 選択した対象の完全な集合を制御側で検証・確保し、1 コマンドで音声側へ渡す。
  コールバックは対象の索引だけを解決し、古い集合を戻りリングへ返して制御側で解放する。
  確定時の解除は計画の採用を待ち、取消は元のフェーダーへ戻す。トラック数の固定上限は持たない
  （[ADR 0228](../decisions/0228-selected-track-live-mix.md)）。

### 7.6 複数のセッション

開いているプロジェクトと履歴試聴用の版は独立したセッションとして持ち、前面セッションだけを短いクロスフェードで出力する。

### 7.7 オフラインのレンダリング

バウンス、フリーズ、書き出しは対象版のスナップショットを `echode` CLI に渡して実行する。
P2 の通常書出しは曲全体のミックス 1 ファイルを初期選択（初期値 WAV・24 bit・プロジェクトのサンプルレート）とし、現在のミュート・ソロ・余韻を反映する
（[書出しの要件](../interaction-design.md#書き出し)）。
曲の終端は音源を仮定せず、長さ無指定時の余韻は初期化時の CLAP tail を使う。
同梱 Sine Pad も同じ経路で処理する（[ADR 0123](../decisions/0123-explicit-sine-pad-plugin.md)）。
CLI の `--tail-frames N` は申告値を N フレームへ置き換え、音源なしでは同じ長さの無音を足す。
`--frames N` は総フレーム数のまま保持し、両指定の併用と u32 の総長の超過は出力作成前に拒否する（[ADR 0250](../decisions/0250-explicit-export-tail.md)）。
Phase 2 のクリップ FX バウンスはクリップチェインの出力と余韻を新しいオーディオの中身・配置として取り込み、トラック FX の二重適用を防ぐ。

現在の CLI はステレオの 32 bit float（既定）と 16 / 24 bit PCM を選べる。
float は 18 バイトの `fmt` とフレーム数の `fact`、PCM24 は 40 バイトの WAVEFORMATEXTENSIBLE を持つ。
PCM16 は形式コード 1・16 バイトの `fmt` を持ち、拡張と `fact` は付けない。
PCM の丸め・飽和と出力保全は [ADR 0247](../decisions/0247-pcm24-wav-export.md)・[ADR 0248](../decisions/0248-pcm16-wav-export.md)、
float の基準音声の保持は [ADR 0243](../decisions/0243-float-wav-metadata.md) に定める。
大容量は RF64 に自動切替し、64 bit の長さ・フレーム数を `ds64`、全形式の `fmt` を WAVEFORMATEXTENSIBLE にする。
RIFF 範囲内のバイト列は変えず、レンダラーの u32 フレーム数は拡張しない（[ADR 0249](../decisions/0249-rf64-wav-export.md)）。
量子化前の TPDF は `--dither tpdf` で明示する。既定はなしで、float との併用は拒否する。
既存の `rand_distr::Triangular` と `rand_chacha` を使い、ファイルごとの固定 seed と連続する左右の別々の標本で分割に依存させない。
仕様と回帰は [ADR 0251](../decisions/0251-tpdf-export-dither.md) に定める。
GUI は書出し要求の順番で記録済みの曲を捕捉し、参照するオブジェクトだけを新しい専用パッケージへコピーする。
CLI はそのパッケージを開くので、元の文書のロックや後続の編集・文書交換から独立する。
設定・寿命・失敗時の境界は [ADR 0254](../decisions/0254-gui-snapshot-export.md) に定める。
CLI の任意の NDJSON と GUI の率・仕上げ中の表示は [ADR 0255](../decisions/0255-export-progress.md) に定める。
`--normalize-peak-db DB` は全ミックスと余韻のサンプルピークから左右共通のゲインを求め、ディザ・量子化前に掛ける。既定は無効。
同じ出力フォルダのリンクを外した一時ファイルへ float を 1 回保存し、再発音せず固定長のバッファで変換する。
対象・設定互換・容量・取消の境界は [ADR 0258](../decisions/0258-peak-export-normalization.md) に定める。
CLI と `render_to_wav` の出力先は元パッケージの外へ限定し、親の正規化・内部を指す既存リンクの検証を復旧より前に行う。
出力の一時物と公開は同じ正規化済みの親を使う（[ADR 0259](../decisions/0259-export-destination-boundary.md)）。
CLI の失敗は `CliError` が引数・I/O の操作とパス・保存・API・音声・再生の原因を分け、`Error::source` から元の原因をたどれる。
`plugin_failure` は装置 ID・CLAP ID・失敗の種類を持つ `PluginBlockFailure` を返し、CLI が表示の直前まで保持する。
公開前の取消と公開後のディレクトリ同期失敗は別の変種で、既存の表示文と終了コードは変えない（[ADR 0305](../decisions/0305-typed-cli-errors.md)）。
GUI の完了結果は固定した出力 URL を保持し、利用者の明示操作だけで Finder へ渡す。
文書交換・次回の失敗／取消との境界は [ADR 0261](../decisions/0261-reveal-export-output.md) に定める。

### 7.8 プレビュー用キャッシュ

編集が止まって数秒たったらトラックのデバイス後・フェーダー前の音を `echode` CLI で裏レンダリングし、`~/Library/Caches/Echode/` に区間ハッシュ鍵で置く（本体は Phase 3）。
OS の数学ライブラリを使う実装では `sw_vers` のプロダクト版も鍵に含める（[ADR 0040](../decisions/0040-libm.md)）。

### 7.9 出力デバイスとトランスポート

- **HAL と時計**: `echode-audio-io` で公開 HAL API の `objc2` 型を使い、制御スレッドが機器一覧・通知・開閉を担当する。
  時計は `ClockSource`（現在は `InternalClock`）で進める（[ADR 0047](../decisions/0047-product-audio.md)）。
  ループとメトロノームはモデルの Undo・ジャーナルから独立させる（[ADR 0055](../decisions/0055-loop-metronome.md)）
- **ループへの進入**: 終端より前から再生した場合だけ終端で戻り、終端ちょうど・以降への開始／シークは通常進行する。
  処理で終端へ到達した場合の折返しは、一時停止と同範囲の計画交換でも保つ（[ADR 0156](../decisions/0156-ruler-loop-range-and-entry.md)）。
- **準備中の取消と保持**: 準備中の取消世代を投入時に共有し、停止後の素材・ヘルパー・出力を再利用する
  （[ADR 0097](../decisions/0097-cancellable-retained-transport.md)）。
  デバイスビューの要求では、再生指令を入れずに出力とヘルパーを開いて同じく保持する（[ADR 0130](../decisions/0130-clap-parameter-editing.md)）。
  最初に開く時は、機器の確認と Workgroup の取得を制御スレッド、ヘルパー起動と素材の準備を準備スレッドで行い、
  準備中の変更を送ってから IOProc を登録する（[ADR 0136](../decisions/0136-output-opening-off-worker.md)）。
  製品アプリはプロジェクトを開いた時に `audio.prepare` で同じ準備を停止のまま始める。
  停止中の編集の素材を読込み中の再生は、素材を載せた計画の公開まで保留する（[ADR 0147](../decisions/0147-output-prewarm-and-background-media.md)）。
  開いた後のヘルパーとの往復はデバイスレーンが待ち、制御側の背景保存とグラフ交換は通信中の制御端点を待たない
  （[ADR 0131](../decisions/0131-device-view-and-responsive-parameters.md)）。
  ライブ停止・一時停止は時計を保持して現在のノートを解放し、配置音声とキャッシュの素材だけを最大 5 ms で減衰する。
  減衰はプラグインの手前で行い、停止中も無音入力でチェイン全体を処理して余韻・自発的な発音をミキサーへ通す。
  マスターの停止ミュートと DSP リセットは行わない（[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
  位置移動・未発音の準備取消とオフライン停止では、次のイベントより前に DSP リセットと全発音停止を送る。
  ライブの破棄する後片付けは完了を待たず、保留中の停止で素材の継続処理を始めない
  （[ADR 0099](../decisions/0099-transport-plugin-state.md)、[ADR 0105](../decisions/0105-nonblocking-stop-cleanup.md)）
- **P2 の開始位置**: プロジェクトに保存する再生開始位置と進行中の位置を分け、停止は開始位置へ戻し、一時停止はその場から再開する
  （[操作要件](../interaction-design.md#再生の開始位置)、[ADR 0117](../decisions/0117-workflow-interactions.md)）。
  セッションが開始位置を持ち、停止は `Rewind` で時計だけを戻してプラグインのリセットと余韻の打ち切りをしない。
  開始位置はパッケージの `transport.json` に保存する（[ADR 0155](../decisions/0155-remembered-playback-start.md)）
- **コールバックの panic**: 出力と試聴のコールバックは panic を捕まえ、そのブロック以降を無音にして出力の失敗として報告する。
  オーディオスレッドではフックが場所と文を前もって確保した領域へ写すだけで、記録へは制御スレッドが書く
  （[ADR 0238](../decisions/0238-audio-callback-panics.md)）
- **panic を起こす書き方の禁止**: 音声スレッドで動くコードを持つ 8 crate（time・realtime・dsp・sine-pad・sandbox・plugin-host・engine・audio-io）は、
  lib のルートの lint で製品コードの添字・スライス・`unwrap`・`expect`・`panic!` を拒否する。
  届かない値は、そのサンプル・ノート・記録を飛ばすか既存のエラーにし、ヘルパーの段が見つからなければ取りこぼしとして減衰で隠す。
  `scripts/check_audio_lints.py` が宣言の欠落と局所の解除を拒否する（[ADR 0284](../decisions/0284-audio-path-panic-lints.md)）
- **出力のエラー**: `echode-audio-io` の出力・試聴・デバイス要求は `AudioError` で失敗を返し、要求の拒否・出力なし・CoreAudio の失敗・出力の停止・
  エンジン・パッケージを変種で区別する。セッションは最後の失敗を型のまま持ち、開けなかった出力の原因をデバイス要求へ返す。
  文字列にするのは状態・試聴の報告の欄と FFI の波形の概観だけで、文面は従来のままである。文書の手番は `DocumentError::Audio` として型のまま運ぶ。
  `scripts/check_typed_errors.py` が `Result<_, String>` の再導入を拒否する（[ADR 0291](../decisions/0291-typed-audio-io-errors.md)）
- **エンジンのエラー**: `PlaybackError` は原因ごとの変種で、ヘルパーの起動・状態の復元・対応の失敗は `Spawn`・`Helper` が `Arc<SessionError>` を型のまま持つ。
  busy・ヘルパーの時間切れ（結果不明）は `SessionError::Refused` の種類で分かれる。文は `Display` だけで作り、従来の文面を保つ
  （[ADR 0294](../decisions/0294-typed-engine-errors.md)）
- **出力のブロックの期限**: 出力の IOProc は、HAL の出力時刻から機器の出力側の安全余白とブロックの 1/16 を引いた時刻にブロックを終える。
  残りは同じ機器の試聴の IOProc と HAL の書込みの分で、入口から 1 ブロックを上限にする。ヘルパーの待ちはそこから 1/8 前まで
  （[ADR 0283](../decisions/0283-output-time-callback-deadline.md)、[ADR 0082](../decisions/0082-callback-deadline.md)）
- **出力デバイス追従**: 既定はシステム出力に追従し、明示選択したデバイスが抜けたときはシステム出力へフォールバックし、戻ったら復帰する
  （[ADR 0015](../decisions/0015-output-device-follow.md)）

## 8. ルーティング

### 8.1 静的チャンネル（X-Y 平面）

各トラックは任意の数のチャンネル（既定はステレオ）を持ち、バスはスピーカーの並びと同じ数の平面を持つ
（[ADR 0039](../decisions/0039-bus.md)）。

### 8.2 コンテナ（Z 軸）

デバイスチェインの中に並列チェインを持つコンテナを置け、出口で自動加算される。入れ子とマクロのフロントパネルに対応する。

### 8.3 センド、レシーバー、サイドチェーン

- **センド / レシーバー / サイドチェーン**: トラックの任意位置から別トラックへ送り、循環はグラフ組立時に拒否する（1 ブロック遅れ明示時のみ許可）
- **P2 のセンド作成**: 送り元ドラッグ・送り先一覧・受け側からの接続元一覧で同じ接続 ID を編集する。
  初期値は FX 後・フェーダー前・0 dB とし、ミュートはセンドも止め、送り元ソロでは関連リターンも開放する
  （[操作要件](../interaction-design.md#センドとレシーブ)）

### 8.4 フォルダ

フォルダトラックは子トラックの出力を受けるバスを兼ね、ネスト深さに上限を置かない（[ADR 0024](../decisions/0024-track-order.md)）。
意味は Reaper と同じで、子の出力を親の入力へ加算し、親自身の音と合わせて親の FX・音量・パンを通す。
トラックの種別を作らず、親の操作と子の操作は互いに保存値を変えない（[ADR 0277](../decisions/0277-reaper-style-folder-bus.md)、未実装）。
P2 のトラック追加位置・フォルダ展開／折りたたみ時の挿入位置・通常複製は
[操作要件](../interaction-design.md#音源を挿して編集に入る) に従う。
選択の下と最上位末尾への空トラック追加、フォルダの開閉と折りたたみ時の追加位置は接続済みとする。
開閉は音声と独立した表示設定であり、フォルダバスは未実装
（[ADR 0127](../decisions/0127-positioned-track-insertion.md)、[ADR 0128](../decisions/0128-folder-disclosure-and-row-projection.md)）。
ミュートは子孫へ継承し、ソロは自身と子孫を対象とする。ミュートをソロより優先し、複数ソロは対象の和集合とする。
継承は制御側で実効フェーダーへ解決する。子の保存値・MIDI 配送は変えず、音量・パン・FX の子への適用はバス化まで未接続とする
（[ADR 0244](../decisions/0244-folder-mute-and-solo-gates.md)）。
通常複製は部分木を独立した中身で写し、複製先は新しいデバイス ID の別インスタンスとして鳴る（[ADR 0129](../decisions/0129-track-duplication.md)）。

### 8.5 フォルダ型ドラムラック（Phase 2 予定）

内蔵ドラムラックはフォルダトラックで表し、親フォルダに MIDI、音ごとの子トラックにサンプラーとサンプルを持たせる
（[ADR 0116](../decisions/0116-folder-drum-rack.md)）。
サンプラーは音量 ADSR を持つゲート再生を既定とし、ワンショット切替と停止・シーク時の発音消去を守る。
通常のドラムパターン複製では MIDI の中身を独立させる。

## 11. オーディオファイルと解析

- **共通サンプルブラウザ（P2）**: フォルダ移動とファイル名検索を併用し、選択で素の音を 1 回試聴、ダブルクリックで選択トラックの再生ヘッドへ配置する
  （[操作要件](../interaction-design.md#共通サンプルブラウザ)）
- **サンプルの試聴**: 文書ごとの試聴スレッドが復号・SRC・専用 IOProc を持ち、文書ワーカーとプロジェクトの出力ストリームを通らない。
  `AuditionAudio` は等倍の素材を先頭から加算し、置換・停止は 5 ms で消す。HAL がプロジェクトの IOProc と加算する
  （[ADR 0135](../decisions/0135-sample-audition.md)）。
  復号・SRC は少しずつ進め、長い素材は有限の先読みリングから全復号前に鳴らす。
  同じ読込みから部分波形を作り、音声側は不足と EOF を区別する
  （[ADR 0151](../decisions/0151-streaming-sample-audition.md)）。
  ブラウザ表示中は無音でも出力を保持し、音量は試聴だけに 5 ms の補間で適用する。
  元のサンプル位置からの要求は制御側で seek して有限の先読みと同じ SRC を通す
  （[ADR 0152](../decisions/0152-browser-audition-controls.md)）。
  直近の要求ごとに操作の種類と、受付・復号・出力の準備・変換・受渡し・発音（HAL の出力時刻）の経過を記録する
  （[ADR 0143](../decisions/0143-browser-click-and-audition-timing.md)）。
  ブラウザの一覧の下の波形は、試聴と別に `sample_overview` が素材の全体を読み、出力も文書も使わない。
  同じブラウザの新しい要求が古い読込みを止め、別のウィンドウのブラウザは止めない（[ADR 0163](../decisions/0163-sample-overview.md)）
- **デコードと書き出し**: AudioToolbox（WAV、AIFF、CAF、MP3、AAC、ALAC、FLAC 等）と Symphonia（Ogg Vorbis 等）を組み合わせる
  正準 PCM の読込みはサンプル数・バイト数・ヘッダ加算を checked 演算で求め、実際の長さと一致する時だけ確保する。
  `read_pcm` の全読込みではサンプルの有限性も検査し、NaN・無限大を含む素材をトリム・SRC の前に拒否する。
  外部音声の一括復号と取込みも全サンプルを検査し、非有限値を保管庫・索引・ピークの書込み前に拒否する。
  有限の振幅をクリップせず、原本を書き換えない（[ADR 0056](../decisions/0056-audio-import.md)）。
- **波形のピーク**: 正準 PCM から CPU で最小・最大のピラミッド（1 段目 64 サンプル）を作り、`cache/peaks/` に置く。
  キャッシュ一致時は正準 PCM を全読込せず、端と 64 サンプル未満の列だけを読み取り専用マップから読む
  （[ADR 0058](../decisions/0058-waveform-peaks.md)、[ADR 0110](../decisions/0110-apple-silicon-draw-and-peaks.md)）
  表示列ごとの tick 境界を再生と同じ `media_sample_at` へ通し、テンポ変化・配置オフセットと EOF 後の無音を保つ。
  1 サンプル未満の列は対応サンプルを参照し、波形を全幅のフレーム等分へ引き伸ばさない。
  素材前・EOF 後・配置外の無音をまたぐ列は、素材内の包絡に 0 を含める。境界が列の途中でも無音を捨てない。
  キャッシュは段構成・実バイト長・有限で順序の正しい最小最大を検証し、不正なら元 PCM から再生成する。
  正常なキャッシュでは引き続き素材の全読込みを省く。
- **タイムストレッチと解析（後続 Phase）**: Signalsmith Stretch（MIT）、Bungee（MPL-2.0）、Rubber Band（GPL-2.0-or-later）を候補とし、テンポ推定・追従は後続 Phase で扱う
  （[後続 Phase の操作要件](../interaction-design.md#オーディオクリップとテンポ)）
