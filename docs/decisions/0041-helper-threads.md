# 0041. ヘルパーはメイン、処理、制御の 3 スレッドに分ける

- 日付: 2026-10-09
- 状態: 採用
- 項目: PLG-5

## 背景

ヘルパーは、プロセスのメインスレッドで共有メモリを待ち、そこで CLAP の `process` を呼ぶ。CLAP は、生成と破棄、`activate` と `deactivate`、`on_main_thread`、GUI をメインスレッドに限る。`start_processing`、`stop_processing`、`process` はオーディオスレッドである。Cocoa のウィンドウもメインスレッドでしか動かない。

`request_callback`、`request_restart`、`request_process` は、どのスレッドから来てもすぐ戻る契約だが、今は空である。`request_callback` は `on_main_thread` をメインスレッドへ予約する。`request_restart` は `deactivate` のあと `activate` で、ホストが遅らせてよい。

Workgroup への参加は、処理をするスレッドが時間制約を付けてから行う（[S3](../spikes/S3-plugin-isolation.md)）。メインスレッドが参加したまま GUI や run loop で止まると、デバイスの Workgroup を塞ぐ。処理スレッドが、時間のかかる `deactivate` や `activate` の完了を待つと、時間制約の計算時間を超えて Workgroup の扱いから外れる。

制御のソケットは PLG-3 が決める。この項目ではソケットを作らない。読むスレッドだけ先に置く。親の再生を始める通知もそこに残す。

## 決定

ヘルパー 1 プロセスの中のスレッドは、次の 3 つである。プラグインの数だけ増やさない。

- **メインスレッド**は、プロセスの `main` である。`CFRunLoop` を回す。CLAP の生成、破棄、`activate`、`deactivate`、`on_main_thread` はここだけで呼ぶ。タイマーもこのループに載せる。Workgroup には入らない。`process` も `start_processing` も `stop_processing` も呼ばない。将来の `NSApplication` は、この同じループを回す。別のスレッドには移さない。ループに入る前に停止が来ても取りこぼさないよう、停止フラグを見てから待ち、待ちの途中でもそのフラグで抜ける
- **処理スレッド**は 1 本である。Workgroup への参加と、デバイス周期の時間制約は、このスレッドだけが行う。参加は共有メモリの待ちに入る前で、ループの準備ができた印はその後に出す。共有メモリの待ち、`start_processing`、`stop_processing`、`process` はここだけで呼ぶ。ロック、メモリの確保、ファイルの読み書きをしない。`deactivate` と `activate` の完了は待たない。親のブロックには、処理できるときは `process` の結果を、再起動の途中では無音を、どちらもその場で返す
- **制御スレッド**は 1 本で、リアルタイムではない。CLAP を直接呼ばない。`request_restart` と `request_process` の順番を進め、PLG-3 の制御の入口もここが読む。Workgroup には入らない。終了フラグで待ちから抜け、メインが処理スレッドと一緒に join する

`request_callback` は、どのスレッドから来てもすぐ戻る。処理スレッドは確保しない。起動時にメインの run loop へ足した `CFRunLoopSource` を起こし、続けて run loop を起こす。`on_main_thread` はその source の実行で、メインスレッドだけが呼ぶ。フラグだけ立てて `CFRunLoopWakeUp` することはしない。

`request_restart` はフラグを立て、処理スレッドの待ちを起こしてすぐ戻る。待ちは共有メモリの警報カウンタで、親のブロックと、ヘルパー内部の再起動・終了の両方が進める。今ブロックの途中なら、そのブロックは最後まで処理して親へ返す。そのあと処理スレッドは `stop_processing` だけ行い、無音で次のブロックに答えながら待ちに戻る。制御スレッドは、処理スレッドがプロセッサを手放してから、メインに `deactivate` と `activate` を実行させる。終わったら処理スレッドを再び起こし、処理スレッドが `start_processing` する。再生が止まっていて親のブロックが来ないときも、この起床で再起動は進む。

`request_process` は、ヘルパーを次のブロックが処理できる状態にする。止まっていれば `activate` と `start_processing` を、上と同じ役割で行う。親にブロックの送信を始めさせる通知は作らない。それは PLG-3 である。

合図は原子変数と、今の共有メモリと同じ起床で渡す。処理スレッドが mutex を取らない。

プラグインのないコピーだけの経路も、処理スレッドが共有メモリを待つ。メインは run loop に入る。制御スレッドは終了を待つ。

`--scan` は今どおりメインスレッドだけで終え、3 スレッドも run loop も起動しない。プロセス内の比較計測も、単一スレッドのままである。

CLAP の thread-pool 拡張は、この項目では実装しない。足すときは、そのスレッドも処理スレッドと同じ Workgroup と時間制約に入れる。

run loop は `objc2-core-foundation` の `CFRunLoop` を使う。手で宣言しない。終了は、処理スレッドが停止を受け取ったあと、応答を返して `stop_processing` し、制御スレッドとメインのループを起こす。メインは両方のスレッドを join してから `deactivate` と破棄をする。

## 結果

メインの run loop と処理スレッドは別である。`request_callback` はメインの `CFRunLoopSource` から `on_main_thread` を呼ぶ。再生が止まっているときの `request_restart` は、親のブロックを待たずに進む。再起動を頼んだブロック自身はゲインのまま返り、次のブロックもタイムアウトしない。処理スレッドは `deactivate` の完了を待たず、そのあいだのブロックは無音で親へ返す。`make check` を通した。
