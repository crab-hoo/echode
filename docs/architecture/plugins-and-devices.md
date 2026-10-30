# プラグイン隔離・スキャン・内蔵デバイスアーキテクチャ（§9、§10）

- 全体索引: [../ARCHITECTURE.md](../ARCHITECTURE.md)
- 操作要件の正本: [../interaction-design.md](../interaction-design.md)
- 現在の実装範囲: [../status.md](../status.md)

## 9. プラグインホスティング

### 9.1 共通の抽象

CLAP、VST3、AU の違いを 1 つのインターフェース（サンプル位置付きイベント処理、パラメータ、ノート表情、状態保存と復元、レイテンシとテール、GUI、プリセット）にまとめる。

| 形式 | 実装 | 補足 |
|---|---|---|
| CLAP | `clack-host` | 主な形式。リモートコントロール、ノート名、ボイス情報、プリセット探索、スレッドプール、ポリフォニック変調等に対応する |
| VST3 | `vst3` crate | Note Expression とサンプル位置付きパラメータ変更に対応する |
| AU | AudioToolbox / AVFAudio | AUv3 と AUv2 を `AUAudioUnit` API で扱う（未実装）。AUv3 は OS が XPC extension で別プロセス実行するためヘルパーを重ねない（[ADR 0111](../decisions/0111-macos-helper-notices.md)） |

### 9.2 プロセスの分け方

設計上はメーカー単位（既定）、プラグイン単位、インスタンス単位、共有の 4 モードを持ち、プラグインごとに上書きできる。
現行製品は 1 デバイス 1 ヘルパーで動かす（[ADR 0049](../decisions/0049-hosted-playback.md)）。

### 9.3 リアルタイムの通信

- **共有メモリと IPC**: オーディオバッファとイベント列は `shm_open`（作成後すぐ unlink し FD を渡す）の共有メモリに置き、ブロックは 2 スロット、入出力イベントは分離して検証してから読む。
  ヘルパーは `posix_spawn` で起動し、`fork` は使わない。状態・`mark_dirty`・再起動通知・段の順番はソケットで運ぶ
  （[ADR 0042](../decisions/0042-plugin-ipc.md)）
- **一括投入と単一締め切り**: 独立したヘルパーには先に一括投入し、そのブロックの単一締め切りで完了したヘルパーから順に回収する。
  同じヘルパーで続くデバイスは 1 往復にまとめる（[ADR 0043](../decisions/0043-plugin-submit-collect.md)）
- **Audio Workgroup の受け渡し**: ヘルパーが `bootstrap_check_in` した名前を制御スレッドが `bootstrap_look_up` し、`os_workgroup_copy_port` の送信権を Mach メッセージで渡す
  （[S3](../spikes/S3-plugin-isolation.md)、[ADR 0111](../decisions/0111-macos-helper-notices.md)）。
  製品の HAL 選択デバイスから Workgroup を取得・保持し、プラグイン追加・再作成・再起動でも引き継ぐ。
  オフライン書き出しはデバイスの Workgroup を使わない（[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)）

### 9.4 クラッシュとハング

- 締め切りまでに応答がなければ対象デバイスだけを無音（短いフェード付き）にして先へ進む
- ヘルパー終了を `kqueue` で検知したら無音化し、デバイスビューに表示して再起動を提案する
- `SIGABRT`、`SIGSEGV`、`SIGBUS`、`SIGILL` でヘルパーが共有領域へ印を書いてから信号を上げ直す
  （[ADR 0079](../decisions/0079-failed-export.md)、[ADR 0111](../decisions/0111-macos-helper-notices.md)）
- 再起動ではヘルパーを起動し直して最後に取得した状態を復元する

- メインスレッドの制御要求は型付きの専有領域で渡し、時間切れ後も完了まで後続を busy で拒否する。
  ソケットの要求と応答は 64 bit の要求番号で照合し、遅れた応答を後続へ貸さない。
  時間切れは取消ではなく結果不明で、自動再送はしない（[ADR 0199](../decisions/0199-owned-helper-control-requests.md)）。

### 9.5 GUI とデバイスビュー

- プラグイン GUI はヘルパープロセスが持つフローティングの `NSPanel` に表示する（[ADR 0132](../decisions/0132-helper-plugin-windows.md)）。
  ヘルパーはメインスレッドで run loop を回し、最初の窓から AppKit のイベントも同じループで処理する。
  音声処理は別スレッド、再起動は制御スレッドが進める（[ADR 0041](../decisions/0041-helper-threads.md)）
- 埋め込み型を優先し、content view を `set_parent` へ渡す。浮動型だけのプラグインはプラグイン自身の窓を `show` / `hide` で扱う。
  開く・前面化はキーウィンドウにせず、中のクリックでだけキーになる。親と同じ作業中のヘルパーが最前面でも表示を保つ
  （[ADR 0061](../decisions/0061-distribution-signing.md)、[ADR 0111](../decisions/0111-macos-helper-notices.md)）
- 列の名前とインスペクターの名前のクリック、候補からの単体追加で開く。
  一括追加・デバイスのコピー・トラック複製では開かない。開閉の往復はデバイスレーンで待つ
- GUI の破棄は明示的な閉鎖で判断し、一時的な非表示で破棄しない。
  ループだけの再準備はヘルパーの状態を復元せず、GUI と未保存値を保つ
  （[ADR 0134](../decisions/0134-plugin-window-lifetime-and-shortcuts.md)）。
- 独自ウィンドウが開いているデバイスは、列とインスペクターの名前の横へ窓アイコンを表示する。
  受信順の寿命通知を編集時の状態から分離し、一時非表示でも開として保つ。表示は保存・Undo の対象外とする
  （[ADR 0138](../decisions/0138-plugin-window-status.md)）。
- ヘッダの先頭名と残りのチェインのメニューも、同じデバイス ID の独自 GUI を開く。
  候補を開く FX 追加ボタンとは分け、オフライン・削除済み・旧文書の名前では開かない（[ADR 0139](../decisions/0139-track-header-fx-names.md)）。
- GUI にフォーカス中の Space と既存の DAW ショートカットは、活動通知から既存コマンドへ中継する。
  Cocoa の文字入力中はプラグインへ渡し、二重配送と長押しの反転を避ける。
- プラグイン側の値とジェスチャーは、処理スレッドが `process` / `flush` の出力から事前確保のリングへ書き、制御スレッドが読み戻した値と文字列つきで親へ送る。
  `request_flush` は停止中も処理スレッドを起こす。ジェスチャー終了は 1 回の Undo、窓が開いている間のジェスチャーなしの変更は静止後に記録する
- 出力を開いた後はトランスポート停止中も無音入力で `process` を送り、プラグイン内の鍵盤と自然な余韻を出力する。
  停止時はノートオフを使い、位置移動・準備取消の DSP リセットと区別する
  （[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)）。
- P2-02 の最初の接続では、ヘッダの FX ボタン、候補の非モーダルパネル、`device.reorder` / `device.copy` を追加した。
  ヘルパー GUI はこの時点では未接続だった（[ADR 0118](../decisions/0118-track-fx-chain-editing.md)）。
  候補の一覧は AppKit の表（`PluginListTable`）で、押した時点で選択し、行のどこでも 2 回目のクリックで追加する
  （[ADR 0162](../decisions/0162-plugin-candidate-table.md)）。インスペクターの一覧の末尾の FX 追加ボタンは要件化のみで未接続である
  （[ADR 0165](../decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)）
- 下部のデバイスビューは選択トラックのデバイスを処理順に横へ並べ、各列に操作・検索欄・全パラメーターの一覧を持つ。
  一覧は画面内の行だけを作り、ピン留めした ID を最上部に固定する。ピン留めはデバイスの設定として保存・Undo・複製する。
  一覧・値の往復は文書ごとのデバイスレーンで待ち、取り込んだ状態の記録だけを文書ワーカーへ戻す
  （[ADR 0131](../decisions/0131-device-view-and-responsive-parameters.md)）。
- 一覧の失敗はその列に表示し、「パラメーターを再読込み」から対象だけを再試行する。
  読込み中の連打を集約し、名前・チェイン操作・他の編集を残す。再試行は状態読込・再起動・保存・Undo を行わない
  （[ADR 0140](../decisions/0140-device-parameter-list-retry.md)）。
- 初回の出力準備は Swift の `AudioState.opening` と既存の状態ポーリングで受け取り、未取得の列の待機表示を分ける。
  取得済みの一覧・オフライン・失敗を優先し、準備完了を一覧の取得成功へ読み替えない
  （[ADR 0141](../decisions/0141-device-output-preparation-status.md)）。
- デバイス列の開閉は文書内だけの表示状態であり、音・ヘルパーの寿命・保存・Undo を変更しない。
  折りたたみ中も名前・バイパス・オフライン・チェイン操作を残し、実際の幅で配置とドロップ位置を決める
  （[ADR 0137](../decisions/0137-device-disclosure.md)）。
- パラメーター編集はデバイスビューの一覧で行う。CLAP の値はモデルへ複製せず、プラグイン状態を正本とする。
  説明と値はヘルパーのメインスレッドで読み、変更は処理スレッドがブロックの合間に `params.flush` で反映する。
  ジェスチャーの終了時に状態を取り込み、`device.set_state` として 1 回の Undo に記録する。
  状態だけの Undo / Redo は同じヘルパーへ記録済みのバイトを読み込み、既定や読込拒否はヘルパーを作り直す
  （[ADR 0130](../decisions/0130-clap-parameter-editing.md)）
- 欄の数値入力は、プラグインの表示単位の文字をヘルパーのメインスレッドで `text_to_value` に渡して読む（`ParseParam`）。
  値・拒否・変換なしを区別し、変換なしのプラグインには数値で入力する。読むだけで値は変えない
  （[ADR 0153](../decisions/0153-value-control-clicks-and-typed-entry.md)）
- トラック CLAP のホストバイパスは FX 行から保存・Undo・再生・書出しへ接続済み。
  FX はドライ音を通し、音源は発音を止めて配置音声を保つ。ヘルパーを保持し、再有効化はリセットと現在のノート追跡を通す。
  バイパスの詳細は [ADR 0120](../decisions/0120-device-bypass.md) を参照する。
- トラック CLAP のオフライン化は記録済み設定とバイパス値を残し、実行チェインから除く。
  旧計画が制御側へ返った後にヘルパーを解放し、再有効化は記録済み状態から新しいインスタンスを生成する。
  素材はドライ通過し、プラグイン解決と状態オブジェクトの読込も省く（[ADR 0125](../decisions/0125-device-offline.md)）。
- Phase 2 のクリップ FX では、デバイスビューの編集対象を「トラック」と「クリップ」で切り替え、クリップ上にも FX ボタンと状態を表示する

### 9.6 スキャンとプラグインブラウザ

- **検知とスキャン**: `/Library/Audio/Plug-Ins/`、`~/Library/Audio/Plug-Ins/`、`CLAP_PATH` を FSEvents で監視し、変更停止 2 秒後に `echode-plugin-host --scan` で差分スキャンする。
  本体プロセスはバンドルを読み込まない（[ADR 0016](../decisions/0016-plugin-scan-is-outside.md)、[ADR 0060](../decisions/0060-plugin-folder-watch.md)）
  子と両パイプは同じ期限で回収し、期限切れは子の状態・各出力の byte 数と開閉を診断へ残す。
  子孫保持等の回帰には、製品へ同梱しない `echode-scan-test-helper` を使い、出力済み・直接の子の終了も確認する
  （[ADR 0190](../decisions/0190-native-scan-deadline-fixtures.md)）。
- **キャッシュと除外**: バンドルパス・`CFBundleVersion`・実行ファイル更新日時とサイズをキーに JSON Lines でキャッシュし、クラッシュ・タイムアウトしたプラグインは除外リストへ記録する
- **ブラウザとチェイン操作（P2）**: ダブルクリック追加時のみ自動で閉じ、ドラッグや Return 追加時は開いたままにする。
  候補の右メニューから選択中の全トラックへ、単一トランザクションで追加する。
  フォーカスと複数選択を分け、操作時の文書・対象を保持する（[ADR 0124](../decisions/0124-selected-track-fx-batches.md)）。
  同一チェイン内の通常ドラッグは並替え、Command＋ドラッグは複製、別トラックへの通常ドラッグは設定コピーとする。
  非同期読込みの追加先は操作開始時の文書と ID で固定する（[操作要件](../interaction-design.md#fx-候補とチェインの操作)）

## 10. 内蔵デバイス

同梱 Sine Pad は実際の CLAP として走査し、通常のデバイスチェインで隔離実行する。
音源のないトラックへ暗黙のシンセを付けず、追加・削除・複製・バイパス・状態保存を共通経路に揃える
（[ADR 0123](../decisions/0123-explicit-sine-pad-plugin.md)）。
`Device` は DSP 単体検査とプラグイン内部の処理用であり、本体へプラグインを読み込む経路ではない。
将来の同梱デバイスにも、暗黙の発音経路を増やさない。

- **Sine Pad とテスト用サンプラー**: 同梱 CLAP 音源は `wavesynth 0.1.0` の Sine Pad、既定サンプラーは `dasp_interpolate 0.11.0` の線形補間（フロアと sinc も選択可）。
  実装 ID と版を記録し、同時発音数に固定上限を置かない。サンプラーは DSP 単体検査用で、製品音源には未接続。
  イベント予算の拡張時はヘルパーの主スレッドで領域を準備し、音声側では発音中の声だけを処理する
  （[ADR 0018](../decisions/0018-test-synth.md)、[ADR 0123](../decisions/0123-explicit-sine-pad-plugin.md)）
- **基本セット（Phase 2）**: ユーティリティ（ゲイン、パン、ステレオ幅、位相、チャンネル入替）、EQ、コンプレッサー、リミッター、ゲート、メーター
- **モジュレーター・コンテナ（Phase 3）**: LFO、エンベロープフォロワー、ADSR、ランダム、数式モジュレーター、コンテナとレシーバー
- **マルチトラック EQ（Phase 4）**: エンジンの解析経路で他トラックのスペクトラムを重ね表示し、帯域衝突箇所を強調する
