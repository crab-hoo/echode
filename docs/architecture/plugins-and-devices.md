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

共有領域に届いた時刻 0 のトランスポートは、翻訳済みイベントを借りて `clap_process.transport` にも渡す。
同ブロック内の後続の変更は元の時刻のまま入力イベント列に残し、時刻 0 がなければ null にする。前ブロックの状態は補わない。
製品の生成・配布は [ADR 0358](../decisions/0358-product-musical-transport.md) の通り、全段へ接続した。

製品は制御側でテンポの整数 index からサンプル境界と局所の容量を準備し、音声側で入力の軌跡から各段の通知を作る。
先頭・ループの各 span・途中のテンポ変更に、小数の拍位置・秒・拍子・小節・ループ範囲を入れる。
停止中は保持位置を時刻 0 に送り、停止時の音源フェードで再生中へ戻さない。
各段の曲時刻は [音声設計書 §7.3](audio-engine.md#73-レイテンシ補正pdc) の入力遅れへ揃える。録音とテンポのランプは未対応である。
ノートも同じグラフの入力遅れへ揃え、`event_cmp` で通知と併合して音源・後段・親 FX へ渡す。
`plugin_notes.rs::Notes` は段ごとの `Cursor` と局所容量の領域を持ち、共有した楽譜から生成する。曲からの表情生成は未接続である。

CLAP の遅延は有効化の直後にメインスレッドで読み、初回のポート情報と再起動通知で親へ送る。
`clap_host_latency.changed` は有効化中の通知だけを受け、毎回の有効化後の問合せで反映する。
親の保持値と活動通知をデバイス列へ運び、出力レートのサンプル数を表示する。
値は保存・Undo の対象外である（[ADR 0246](../decisions/0246-plugin-latency-reporting.md)）。
再生と書出しは、報告値で合流点を揃える（[ADR 0339](../decisions/0339-plugin-delay-compensation.md)）。

`clap.thread-check` は、生成したスレッドの `ThreadId` と既存の処理スレッドの TLS で答える。
初期化・有効化・メイン callback は前者、start・process・reset・stop は後者で判定し、プラグインの補助スレッドを音声扱いしない。
問合せはロック・確保・IPC をしない。比較用の同一スレッド処理も音声区間を明示する（[ADR 0347](../decisions/0347-clap-thread-check.md)）。
`clap.log` は任意スレッドの診断を固定量のスロットへ写し、制御側だけがプラグイン ID・重大度付きで stderr へ書く。
先頭 1,024 byte の切詰めと、満杯・競合の破棄を明示し、音声側では確保・ロック・書込みをしない。
詳細は [ADR 0349](../decisions/0349-clap-log-relay.md)。
`clap.timer-support` は主スレッドの handler が登録を持ち、ヘルパーの主ループが最初の期限まで待って通知する。
停止中・窓なしでも動き、callback 中の自己解除・後続の解除・追加を許す。遅れた周期をまとめて再送しない。
登録表はインスタンスとともに破棄し、音声スレッドへ時計・確保・ロックを足さない（[ADR 0351](../decisions/0351-clap-main-thread-timers.md)）。
通知は主ループの進行に依存し、実時間の精度は保証しない。
`clap.tail.changed` は主スレッドへ中継して最新値を読み、再有効化後も更新する。
親は `PluginControl` の共有 atomic に値を集約し、ゼロと無限値も反映する。
書出しの確定前には番号付きの問合せで読み直す（[ADR 0352](../decisions/0352-live-clap-tails-and-export-fence.md)）。
`clap.audio-ports` は 6 つの単一フラグすべてを対応と答え、名前・フラグ・型・in-place ペア・ID の変更では共有領域を変えない。
音声は常に別々の f32 バッファを使い、名前だけの通知では再起動しない。
入出力ごとのチャンネル数の列（`PortLayout`）は有効化ごとに読み直して処理器へ渡し、`Remap` も対象の構成を運んで一致するブロックだけを処理する（不一致の間は無音でイベントを捨てる）。
有効化で `PortLayout` が変われば `PortsChanged` で親へ知らせ、エンジンは最新の構成でルーティングの検証と共有領域の準備を行い、計画の採用で両者を同時に切り替えてリセットとノートの再送を行う。
トラックが受けられない構成は `BusMapping` で計画を断る（[ADR 0382](../decisions/0382-audio-port-changes-exchange-mapping-and-routing.md)）。
`clap.audio-ports-config` の ID による選択は、既存の再起動世代で処理器を止め、無効化中の主スレッドで実行する。
明示した一覧要求（`count`・`get` と config-info）は CLAP の `[main-thread]` なので、処理器を動かしたまま主スレッドで読む（[ADR 0389](../decisions/0389-live-clap-audio-config-listing.md)）。
一覧は保持せず、`rescan` は次の要求で反映する。一覧の読込みでは音声を中断しないが、暗黙には読まない。
選択後と拒否後は共通の再有効化で正確なポートを読み直し、`PortsChanged` を応答より先に送る。
成功した選択は dirty にし、保存はプラグイン自身の状態に依存する（[ADR 0384](../decisions/0384-clap-audio-port-config-selection.md)）。
`clap.audio-ports-config-info`（`/1` と互換 `/draft-0`）がある時は現在選択中の構成 ID と各構成の個別ポート詳細も読み、無い時は未設定のまま一覧を返す。
`clap.configurable-audio-ports`（`/1` と互換 `.draft1`）による任意変更の照会（`can_apply`）と適用（`apply`）は、選択と同じく無効化中の主スレッドで実行する（[ADR 0385](../decisions/0385-clap-configurable-audio-ports-and-config-info.md)）。
構成メニュー UI は [ADR 0386](../decisions/0386-device-audio-configuration-controls.md) で接続した。
デバイス列の明示ボタンだけで一覧を読み、現在 ID がある時だけ選択中を示す。文書の publication 後は一覧を破棄し、暗黙には再取得しない。
`device.audio_config.select` は背景保存を保留してから、デバイスレーンで ID 選択と状態取得を続けて行う。
文書ワーカーの共通の `commit` が取得した状態を単一の Undo 編集へ記録し、失敗でも保留を解く。
選択 ID や任意の要求列は別に保存せず、プラグイン自身の状態・通常の保存と復元を正本にする。
任意変更は「音声ポートを変更」の一時フォームで入出力・0 始まりの番号・チャンネル数（0 は無効化・削除）を入力し、全行を一度の `device.audio_ports.apply` と状態編集へ送る。
入力・取消・フォームの表示はプラグインを呼ばず、publication 後や取消済みのフォームからは適用しない（[ADR 0387](../decisions/0387-device-audio-port-change-form.md)）。
可否の事前照会はせず、適用の拒否を表示する。
標準の `port_details` は [ADR 0388](../decisions/0388-typed-clap-audio-port-details.md) の所有する型でフォームから送る。
`details` がなければ従来どおりチャンネル数からモノ／ステレオだけを推測する。
サラウンドはバッファ順のチャンネルコード、Ambisonic は順序と正規化を明示し、ヘルパーだけが型に合う借用のポインターを作る。
配置の長さ違い・空のサラウンド配置は呼出し前に拒否する。無効化・削除は自動モードの 0 を使い、意味上の可否はプラグインが判定する。
配置を別に保存せず、プラグイン状態・既存のポート交換と単一 Undo の経路を使う。多チャンネル母線やチャンネルの並替えは追加しない。
`clap.note-ports` は native CLAP・MIDI 1.0・MIDI 2.0・MPE を対応と答える。
入力形式はこの順で選び、既存の経路を保つ（[ADR 0375](../decisions/0375-midi-1-notes-for-midi-only-clap-ports.md)、[ADR 0379](../decisions/0379-midi2-notes-for-clap-ports.md)）。
MIDI 2.0 はグループ 0 の UMP で、16 bit のベロシティ（0 も発音）と 32 bit のポリプレッシャーを送る。
両 MIDI 形式ともノート ID・音程と明るさの表情・ワイルドカードのチャンネルと音高は送れず、同じ音高の重なりを別 ID では扱えない。
MPE は他の 3 形式を受けないポートだけに使い、15 メンバーの下位ゾーンを RPN 6 で設定してから、ノートをメンバーチャンネルの MIDI 1.0 で送る。
空いたメンバーは最初のノートが ID で所有し、15 音までは同じ音高でも共有しない。16 音目からは最も空いたチャンネルを共有する。
割当は `MpeZone` の固定量の表で数え、ノートオフは元のチャンネルへ送り、始めていないノートオフは送らない。
表情の曲からの生成は未接続である。MPE の受信・変換境界は [ADR 0381](../decisions/0381-mpe-note-expression.md) の通りに扱う。
単独所有中のノート ID にだけ、音程をピッチベンド、圧力をチャンネルプレッシャー、明るさを CC74 で送る。
処理開始・リセットごとに RPN 0 で全メンバーの幅を ±48 半音にし、設定と変換は同じ定数を使う。
ノートオンごとに音程中央・圧力 0・明るさ中央を先行させる。変換の最大展開（1 入力から 4 出力）は制御側で準備する。
共有開始時は所有ノートも中立へ戻り、共有中は表情を送らない。共有が終わっても捨てた表情は再送しない。
全発音停止は、対応する形式ごとに全 16 チャンネルへ CC120 を送り、曲のイベントより先に借用でつなぐ。
MPE のゾーンの設定は処理の開始ごととリセットごとにその後へ続け、割当表も消す。変換と割当は 1 ブロックに 1 回だけ行う。
名前は保持・表示せず、名前だけの通知では音声を再起動しない。
全走査の通知は初期化と無効化中に受け、入力ポートから作る全発音停止と MIDI 変換の一覧（`NoteInputs`）を毎回の有効化前に読み直す。
一覧は処理器と一緒に所有権を渡し、音声側では差替え・解放・拡張をしない（[ADR 0356](../decisions/0356-clap-note-port-capabilities-and-lifecycle.md)）。

### 9.2 プロセスの分け方

設計上はメーカー単位（既定）、プラグイン単位、インスタンス単位、共有の 4 モードを持ち、プラグインごとに上書きできる。
現行製品は 1 デバイス 1 ヘルパーで動かす（[ADR 0049](../decisions/0049-hosted-playback.md)）。
親はヘルパー 1 つにつき記述子を 4 つ、スレッドを 2 つ持つ。最初のヘルパーの前に、記述子の soft の上限を上げる（[ADR 0237](../decisions/0237-helper-descriptor-limit.md)）。

### 9.3 リアルタイムの通信

- **共有メモリと IPC**: オーディオバッファとイベント列は `shm_open`（作成後すぐ unlink し FD を渡す）の共有メモリに置き、ブロックは 2 スロット、入出力イベントは分離して検証してから読む。
  共有領域の交換は、古い領域の公開済みの世代をすべて処理してから行い、交換の直前のイベントも届ける（[ADR 0341](../decisions/0341-recover-notes-after-helper-overrun.md)）。
  ヘルパーは `posix_spawn` で起動し、`fork` は使わない。状態・`mark_dirty`・再起動通知・段の順番はソケットで運ぶ
  （[ADR 0042](../decisions/0042-plugin-ipc.md)）
  ソケットで渡す記述子は `Remap` の 1 メッセージに 1 個だけである。受け手は届いた記述子を全部持ち、2 個以上や切り詰めがあれば全部閉じて、そのソケットの受信を失敗させ続ける
  （[ADR 0288](../decisions/0288-received-descriptors-are-all-owned.md)）。
  違反は `DescriptorRuleError` で、届いた記述子の数とともに後の受信でも同じ原因を返す。
  送り手は `send_tagged` でも `Remap` の記述子を渡す（[ADR 0317](../decisions/0317-typed-control-transport-errors.md)）。
  1 枠の長さは 64 MiB までで、本体がそれを超えるメッセージは同じ要求 ID の `PART` の枠に分け、最後の枠だけが種類と記述子を持つ。
  受け手はつないでから解釈し、途中に別の要求の枠が来れば `ControlDecodeError::InterleavedPart` で拒否する。メッセージ全体の上限は置かない。
  受信は待たずに読める分を 1 回 1 枠分まで読み、各端の送受信の領域は 1 MiB である（[ADR 0318](../decisions/0318-split-control-messages.md)）。
  記述子は非ブロックで、送信は余地がない間に相手の分を読んで `ready` に置くので、両端が同時に長いメッセージを送っても進む。
  本体の要求は送信と応答の待ちで 1 つの期限を共有する。期限で終わった送信は `ControlSendError` で、途中で切れたら送る側を閉じて以後は送らない。
  ヘルパーの送信は期限を持たず、背景の保存の要求は待たない（[ADR 0319](../decisions/0319-control-send-deadline.md)）。
- 共有メモリの作成の失敗は `RegionError::SharedMemory` で操作と元の I/O エラーを保持する。
  `SharedMemoryOperation` で open・unlink・サイズ変更を区別し、errno は `Error::source` からも読める。
  表示文と FD の受渡し形式は変えない（[ADR 0311](../decisions/0311-typed-shared-memory-errors.md)）。
- 制御メッセージの窓名とパラメーターの UTF-8 は共通の `control_text.rs::read_text` で検証する。
  不正なら `InvalidData` の `io::Error` の内側に `ControlTextError` を保持し、窓名とパラメーターを型で区別する。
  元の `Utf8Error` は `source` から読める。位置は各文字列内で、表示文・通信の形式は変えない（[ADR 0312](../decisions/0312-typed-control-text-errors.md)）。
- 拒否以外の制御メタデータが不正なら、`ControlDecodeError` を `InvalidData` の `io::Error` の内側に保持する。
  `ControlField` と必要・実際のバイト数、未知の種類・コマンド、ポートの方向と列の長さ、フレームの宣言長と許容範囲を型で読める。
  公開操作の `SessionError::Io` でも同じ原因を保ち、表示文と受理する通信の形式は変えない（[ADR 0315](../decisions/0315-typed-control-metadata-errors.md)）。
- パラメーター・通知・窓の報告の内側の構造が不正なら、同じく `PayloadDecodeError` を保持する。
  切詰めは `ParamPayload`・`ParamField`・項目の番号・必要と残りのバイト数、値とコードの不正は受けた値を型で読める。
  列は `payload_decode.rs` の `Cursor::list` だけで読み、項目の番号と不正な数からの確保の上限を一か所で決める（[ADR 0316](../decisions/0316-typed-parameter-payload-errors.md)）。
- **一括投入と単一締め切り**: 独立したヘルパーには先に一括投入し、そのブロックの単一締め切りで完了したヘルパーから順に回収する。
  同じヘルパーで続くデバイスは 1 往復にまとめる（[ADR 0043](../decisions/0043-plugin-submit-collect.md)）
- **Audio Workgroup の受け渡し**: ヘルパーが `bootstrap_check_in` した名前を制御スレッドが `bootstrap_look_up` し、`os_workgroup_copy_port` の送信権を Mach メッセージで渡す
  （[S3](../spikes/S3-plugin-isolation.md)、[ADR 0111](../decisions/0111-macos-helper-notices.md)）。
  ヘルパーは受け取った後に受信権を手放し、名前の登録を消す。Mach の束縛は mach2、Workgroup の宣言は `os_workgroup.rs` にある（[ADR 0326](../decisions/0326-foreign-declarations-checked-against-the-sdk.md)）。
  製品の HAL 選択デバイスから Workgroup を取得・保持し、プラグイン追加・再作成・再起動でも引き継ぐ。
  オフライン書き出しはデバイスの Workgroup を使わない（[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)）
- **メインへの通知**: 処理スレッドは `MainSignal` の保留ビットを立て、既存の `control_alarm` だけを起こす。
  制御スレッドがビットを先に消費してから `CFRunLoopSourceSignal`・`CFRunLoopWakeUp` を呼ぶ。
  `request_callback`・起動要求・GUI の通知を同じ入口で中継し、通常の制御ループと再起動・メイン応答の待ちでも取り出す。
  メイン・制御・プラグインのほかのスレッドからの通知は直接起こす。処理スレッドはメインを待たない（[ADR 0343](../decisions/0343-relay-realtime-main-notifications.md)）。

### 9.4 クラッシュとハング

- 締め切り超過・処理失敗は、後段を含む対象トラック出力だけを直前の標本から 5 ms で無音化し、復帰も現在の出力から 5 ms でつなぐ。
  連続した失敗では減衰をやり直さず、計画交換でも状態を保持する。失われた処理の復元や待ちの期限変更はしない
  （[ADR 0245](../decisions/0245-plugin-deadline-fades.md)）。
- イベントを公開できなかった段と、上流の失敗でそのブロックを受け取れなかった後段は、次の公開成功時にリセットして現在のノートを追い直す。
  公開済みで遅れただけの段や、イベントのない欠落ではリセットしない。回復の保留は計画交換でも引き継ぐ。
  追い直しは対象トラックの生きた集合だけを読み、ループ頭では古い集合を鳴らさない。
  古いイベントや音声は再送せず、余韻も復元しない（[ADR 0341](../decisions/0341-recover-notes-after-helper-overrun.md)、UC-75）。
- ヘルパー終了を `kqueue` で検知したら無音化し、デバイスビューに表示して再起動を提案する
- 段の出力の NaN・無限大は、その標本から 0 にして次の段へ渡し、トラックを自動ミュートする。再生の開始で解除してリセットする（[ADR 0282](../decisions/0282-plugin-nan-and-master-auto-mute.md)）
  装置自身の出力は遅延線の前、音源と配置音声・子の和は合流の後で調べる。
  合流で生じた非有限値は装置なし（`device: None`）とし、正常な音源を障害扱いしない。
  `render_failure` がメーターの同じ記録から型付きの書出し拒否を返し、アプリ・ログも合流を原因とする（[ADR 0345](../decisions/0345-track-mix-fault-attribution.md)）。
- 有限の値の積と和が f32 を超えても 0 にしない。フォルダバスの f32 の面へ収まらない子の和は、その標本から面を 0 にして親の装置なしの障害にする。
  マスターでは無限大のまま機器のガードがその標本から止め、書出しは `RenderFailure::NonFiniteMaster { at }` で拒否する。
  判定は `output_guard.rs` の一つの述語（`first_fault`・`first_nonfinite`）で、減衰はヘルパーの有無によらず `PluginDeclick::conceal` である
  （[ADR 0359](../decisions/0359-finite-overflow-is-a-fault.md)）。
- `SIGABRT`、`SIGSEGV`、`SIGBUS`、`SIGILL` でヘルパーが共有領域へ印を書いてから信号を上げ直す
  （[ADR 0079](../decisions/0079-failed-export.md)、[ADR 0111](../decisions/0111-macos-helper-notices.md)）
- 再起動ではヘルパーを起動し直して最後に取得した状態を復元する
  共有領域は毎回新しく作り、世代番号と親だけが持つ破棄済みの世代を旧領域から引き継がない。
  既存領域を初期化して再利用する入口は持たない（[ADR 0364](../decisions/0364-remove-unused-region-restart-reset.md)）。
  準備・状態復元に失敗した新しいヘルパーは、エラーを返す前に終了・回収し、終了監視を join する。
  ホストの破棄でも終了要求の成否にかかわらず監視を join し、共有領域と fan-in の生ポインタの寿命を閉じる
  （[ADR 0362](../decisions/0362-published-media-and-helper-lifetimes.md)）。
- 出力を閉じる・機器を替える時は、起動中のヘルパーを取消の印（`SpawnOptions::cancel`）で止める。
  起動を待つスレッドが自分で子を止めて `Cancelled` を返し、次の起動も始めない（[ADR 0334](../decisions/0334-cancel-helper-starts-on-close.md)）

- メインスレッドの制御要求は型付きの専有領域で渡し、時間切れ後も完了まで後続を busy で拒否する。
  ソケットの要求と応答は 64 bit の要求番号で照合し、遅れた応答を後続へ貸さない。
  時間切れは取消ではなく結果不明で、変更する要求は自動再送しない（[ADR 0199](../decisions/0199-owned-helper-control-requests.md)）。
  読取りだけの背景の状態取得は、busy・時間切れ・通信失敗でも未取得の印を保ち、1 秒以上空けて新しい番号で再試行する。
  プラグイン自身の拒否・拡張の欠落・停止などはその取得だけを終え、実行中に届いた新しい変更の印は消さない。
  失敗は成功までの最初の 1 回を原因・要求番号・再試行の有無つきで警告し、成功かヘルパーの再起動で間引きを解除する。
  復元・値の変更・同期の確定は再送しない。毎回 2 秒を超える保存は、取得できないまま再試行を続ける
  （[ADR 0348](../decisions/0348-background-state-capture-retries.md)）。
  同期の確定が実行中の背景取得を引き取った場合、その確定の取得が失敗しても同じ未取得の印・再試行・警告の規則へ戻す。
  背景取得の回収待ちの失敗と、回収後の同期取得の失敗を一つの完了入口で扱う。
  背景取得を引き取っていない同期の確定は、自動の背景取得を追加しない。
  ヘルパーの拒否は種類付きの `Refusal` で送る。busy（始めていない）と、ヘルパー自身の時間切れ・停止（結果不明）と、ほかの拒否（ヘルパーの文）を型で分ける。
  ホストではどれも `SessionError::Refused` になる（[ADR 0293](../decisions/0293-typed-helper-boundary-errors.md)）。
  プラグイン自身の失敗（状態・窓・ポートの拡張がない、`false` で断った、ポートの記述が使えない）は `Refusal::Plugin(PluginFailure)` で、ヘルパーの内部と通信路で同じ型である。
  起動の失敗は `Refusal::Launch(LaunchFailure)` で、原因の種類・errno・NUL の位置・CLAP の版と元の診断文を分けて送る。
  clack の型への対応はヘルパー側だけに置き、本体はローダーへ依存せず、文面から原因を分類しない（[ADR 0308](../decisions/0308-typed-helper-launch-failures.md)）。
  起動拒否自体が壊れていれば `LaunchDecodeError` を `io::Error` の内側に保持する。
  不足した `LaunchField` と長さ、未知コード・不正フラグ、診断文の元の UTF-8 の原因を分け、表示文と通信形式は変えない（[ADR 0313](../decisions/0313-typed-launch-refusal-decoding.md)）。
  拒否の残る構造の不正は `RefusalDecodeError` で、欠けた／未知の種類・余分なバイト・固定長の不一致・不正な応答コードとポートの向きを保持する。
  固定長は `RefusalField` と必要・実際のバイト数で区別する。表示文と受理する通信の形式は変えない（[ADR 0314](../decisions/0314-typed-refusal-structure-errors.md)）。
  予約済み CLAP ID は `Refusal::InvalidParameterId` で拒否し、一括変更を適用しない。
  内部の応答不一致は `Refusal::UnexpectedMainReply` で期待・実際の `MainReplyKind` を送る。診断文と再送しない判断は変えない（[ADR 0309](../decisions/0309-typed-helper-request-failures.md)）。
  起動の返信は読込み・生成・起動専用の `LoadError` だけを受ける。
  未分類の `Refusal::Other` はなく、旧種類 `0` のフレームも拒否する（[ADR 0310](../decisions/0310-typed-plugin-load-boundary.md)）。

- 制御専用の `PluginControl` は、`session_operations.rs` に型付きの操作と通信の実装を直接持つ。
  `ControlState` は要求番号・通知・保存の管理を共用し、ホスト経由の保存・復元の別経路は持たない。
  再起動と回帰も同じハンドルへつなぐ（[ADR 0227](../decisions/0227-control-only-plugin-operations.md)、SPG-06）。
  `Dirty` 通知は `dirty_notice` へ畳み込み、未使用の `RequestProcess` は送受で落とす。
  `PluginRestarted`（最大 64）と `inbox`（最大 64）・1 回の `drain`（最大 64 件）を区切り、`poll_state_save` は再開通知の前に溜まった変更も 1 回で引き取る（[ADR 0367](../decisions/0367-helper-notice-bounds-and-audit-diagnostics.md)）。

### 9.5 GUI とデバイスビュー

- プラグイン GUI はヘルパープロセスが持つフローティングの `NSPanel` に表示する（[ADR 0132](../decisions/0132-helper-plugin-windows.md)）。
  ヘルパーはメインスレッドで run loop を回し、最初の窓から AppKit のイベントも同じループで処理する。
  音声処理は別スレッド、再起動は制御スレッドが進める（[ADR 0041](../decisions/0041-helper-threads.md)）
  再起動の停止の確認とメインの完了が要求世代へ揃うまで、処理の開始と `request_process` の消費を止める。
  メインが停止済みプロセッサを受け取れなければ、完了扱いにせずヘルパーを止める。
  制御側は再起動の間、ソケットではなく内部の警報を待ち、親の要求を後で読む。
  通常のソケット待ちの 20 ms ポーリングは残る（[ADR 0342](../decisions/0342-restart-completion-gates-processing.md)）。
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
  記録を待つ間、ワーカーは保存の保留を無関係な文書の変更の後も保ち、セッションはそのデバイスの経路へヘルパーの適用済みのバイトを渡してヘルパーを残す。
  Undo / Redo は静止を待つ変更を先に記録する（[ADR 0229](../decisions/0229-plugin-window-live-edits.md)）。
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
- 初回の出力準備は Swift の `AudioStatus.opening` と既存の状態ポーリングで受け取り、未取得の列の待機表示を分ける。
  取得済みの一覧・オフライン・失敗を優先し、準備完了を一覧の取得成功へ読み替えない
  （[ADR 0141](../decisions/0141-device-output-preparation-status.md)）。
  開いた出力の置換の準備は `AudioStatus.updating` で同じく分ける。判定は新しいデバイスの要求を待たせる条件と同じである（[ADR 0336](../decisions/0336-device-columns-show-output-updates.md)）。
- デバイス列の開閉は文書内だけの表示状態であり、音・ヘルパーの寿命・保存・Undo を変更しない。
  折りたたみ中も名前・バイパス・オフライン・チェイン操作を残し、実際の幅で配置とドロップ位置を決める
  （[ADR 0137](../decisions/0137-device-disclosure.md)）。
- パラメーター編集はデバイスビューの一覧で行う。CLAP の値はモデルへ複製せず、プラグイン状態を正本とする。
  説明と値はヘルパーのメインスレッドで読み、変更は処理スレッドがブロックの合間に `params.flush` で反映する。
  通知の中継に使う説明のフラグは主スレッド内で共有し、プラグインを呼ぶ間はキャッシュを借用しない。
  説明の読取り中の `params.rescan(INFO / ALL)` は世代を進め、その読取りの結果を次回へ残さない
  （[ADR 0357](../decisions/0357-reentrant-parameter-description-rescans.md)）。
  ジェスチャーの終了時に状態を取り込み、`device.set_state` として 1 回の Undo に記録する。
  状態だけの Undo / Redo は同じヘルパーへ記録済みのバイトを読み込み、既定や読込拒否はヘルパーを作り直す
  （[ADR 0130](../decisions/0130-clap-parameter-editing.md)）
  確定の完了入口は取得の結果全体を受け、取得・記録の成否にかかわらず対象の編集中の印を先に外す。
  失敗はそのまま報告し、再送や Undo は行わない。保留していた背景保存と以後の背景保存は通常の保存へ戻す
  （[ADR 0346](../decisions/0346-failed-device-commits-release-saves.md)）。
- 記録済みの状態は保管庫の名前（BLAKE3）で表す。置換と Undo はヘルパーの適用済みの状態と名前で比べ、
  同じならバイトを読まない。読むのは新しいヘルパーと状態の変わったヘルパーだけで、背景の準備ではそのスレッドで読む
  （[ADR 0329](../decisions/0329-plugin-states-by-name.md)）
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
- **キャッシュと除外**: バンドルパス・`CFBundleVersion`・実行ファイル更新日時とサイズをキーに JSON Lines でキャッシュする。
  ヘルパーが最後まで動いて拒んだ版だけを `rejected` の行で除外する。時間切れと本体側の失敗は記録せず、次に走査し直す。
  常駐のカタログは 5 秒で全部を走査して一覧を出した後、時間切れだけを 120 秒で走査し直す。
  走査のヘルパーは親を失ったら終わる（[ADR 0285](../decisions/0285-scan-timeouts-are-not-verdicts.md)）。
  `ScanError` の段（判定・時間切れ・読めない）は中身も型で、終了状態・stderr・管の量・`io::Error` を持つ（[ADR 0295](../decisions/0295-typed-plugin-host-errors.md)）
  既存の走査キャッシュ読込み（`load_existing_scan_cache`）は、`NotFound` を未作成として静かに空へ返しつつ、破損 JSON や権限・種別不正は `echode_plugin_host::scan` へパスと原因を警告記録する（[ADR 0367](../decisions/0367-helper-notice-bounds-and-audit-diagnostics.md)）。
  FFI の目録も `CatalogError` で、ヘルパーの未検出（指定パスを保持）・元の目録エラー・JSON を分ける。
  C ABI では従来の成功・容量不足・失敗の数値だけを返し、内部で文へ戻さない（[ADR 0302](../decisions/0302-typed-ffi-catalog-errors.md)）。
- **ブラウザとチェイン操作（P2）**: ダブルクリック追加時のみ自動で閉じ、ドラッグや Return 追加時は開いたままにする。
  候補の右メニューから選択中の全トラックへ、単一トランザクションで追加する。
  フォーカスと複数選択を分け、操作時の文書・対象を保持する（[ADR 0124](../decisions/0124-selected-track-fx-batches.md)）。
  同一チェイン内の通常ドラッグは並替え、Command＋ドラッグは複製、別トラックへの通常ドラッグは設定コピーとする。
  非同期読込みの追加先は操作開始時の文書と ID で固定する（[操作要件](../interaction-design.md#fx-候補とチェインの操作)）

## 10. 内蔵デバイス

同梱 Sine Pad は実際の CLAP として走査し、通常のデバイスチェインで隔離実行する。
音源のないトラックへ暗黙のシンセを付けず、追加・削除・複製・バイパス・状態保存を共通経路に揃える
（[ADR 0123](../decisions/0123-explicit-sine-pad-plugin.md)）。
`Device` はエンジンの DSP 単体検査用であり、本体へプラグインを読み込む経路ではない。
将来の同梱デバイスにも、暗黙の発音経路を増やさない。
`echode-dsp::SinePad` を同梱 CLAP とエンジンの検査用アダプターで共有し、音源からホストのエンジンへの依存を持たない。
確保の検出も `echode-dsp` に置き、プラグイン固有の TLS 初期化を維持する（[ADR 0266](../decisions/0266-host-independent-sine-pad-dsp.md)）。

- **Sine Pad とテスト用サンプラー**: 同梱 CLAP 音源は `wavesynth 0.1.0` の Sine Pad、既定サンプラーは `dasp_interpolate 0.11.0` の線形補間（フロアと sinc も選択可）。
  実装 ID と版を記録し、同時発音数に固定上限を置かない。サンプラーは DSP 単体検査用で、製品音源には未接続。
  イベント予算の拡張時はヘルパーの主スレッドで領域を準備し、音声側では発音中の声だけを処理する
  （[ADR 0018](../decisions/0018-test-synth.md)、[ADR 0123](../decisions/0123-explicit-sine-pad-plugin.md)）
- **基本セット（Phase 2）**: ユーティリティ（ゲイン、パン、ステレオ幅、位相、チャンネル入替）、EQ、コンプレッサー、リミッター、ゲート、メーター
- **モジュレーター・コンテナ（Phase 3）**: LFO、エンベロープフォロワー、ADSR、ランダム、数式モジュレーター、コンテナとレシーバー
- **マルチトラック EQ（Phase 4）**: エンジンの解析経路で他トラックのスペクトラムを重ね表示し、帯域衝突箇所を強調する
