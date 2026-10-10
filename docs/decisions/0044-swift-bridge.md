# 0044. Rust と Swift は、生成した C ABI と薄い Swift の層でつなぐ

- 日付: 2026-10-10
- 状態: 採用
- 項目: UI-2

## 背景

ARCHITECTURE §13.3 は、命令に UniFFI、描画の大きな配列に C ABI を予定していた。Swift 6.4 の strict concurrency と生成コードの相性、スナップショットの寿命は未検証だった。

`spikes/bridge` に、同じ Rust のドキュメント用スレッドを呼ぶ 2 案を作った。UniFFI 0.32.2 は、非同期関数、小さいレコード、同期コールバック、非同期コールバックを生成する。C ABI は cbindgen 0.29.4 でヘッダを生成し、Swift の `CheckedContinuation` と C の完了コールバックで包む。生成コードは加工せず、両案とも `-swift-version 6 -strict-concurrency=complete -warnings-as-errors` でコンパイルする。

両ライブラリは MPL-2.0。ソースに適用された「Incompatible With Secondary Licenses」の通知はなく、GPL-3.0-or-later と両立する。cbindgen の LICENSE にある Exhibit B のひな形は、ソースへの適用通知ではない。生成器を含む依存は `cargo deny --all-features check licenses bans sources` でも確認する。

## 検証結果

環境は arm64、macOS 27.0（26A428）、Apple Swift 6.4（swiftlang-6.4.0.34.1）、Rust 1.96.1。Rust は release、Swift は `-O`。2026-10-10 の 1 回の実行で、次の結果になった。時間に合格の閾値は置かない。

- 両案の通常の非同期関数と同期コールバックは、コンパイルと実行を通った。32 件を別の Swift Task から送っても、版は欠けずに順番に進む。通知は `AsyncStream` を通して `MainActor` で受ける。コールバックからスナップショットを読み直してもデッドロックしない。空の名前はエラーとなり、版は変わらない
- UniFFI の非同期コールバックだけは、生成された `uniffiTraitInterfaceCallAsync` と `uniffiTraitInterfaceCallAsyncWithError` の `Task` で `SendingClosureRisksDataRace` となった。Swift 5 モード、警告の抑制、生成物の手直しで回避していない。通常の非同期関数まで失敗したわけではない
- 両案とも、不変の `Arc<Project>` を持つスナップショットを複製せずに保持できた。同じ版の同一性は `Arc::ptr_eq` で確かめる。新しい版を公開したあとも、別の Swift Task が古い版を読める。古いハンドルを解放すると、Rust 側で数えた保持中のスナップショットは 2 個から 1 個へ戻る。ドキュメントの Swift オブジェクトを解放したあとも、保持したスナップショットは読める
- C ABI は、Rust が 1 回だけ用意した読み取り専用のノート配列を、ポインタと件数で借りて読めた。UniFFI の `Vec<BridgeNote>` の戻り値は、Rust の配列を複製し、FFI のバッファへ直列化してから、新しい Swift の配列へ復元する。UniFFI のオブジェクトハンドルの共有と、配列の複製は別の性質である
- 10 万要素、1 要素 24 byte の検証用配列で、1,000 回の C ハンドル取得と借用した配列の 1 要素の読み取りは 0.104250 ms。UniFFI の 1,000 回のオブジェクト取得と件数の読み取りは 0.035750 ms。読み取る値が違うため、呼び出し速度の優劣を決める比較には使わない
- 同じ配列の `start` をすべて加算する処理を 100 回行うと、C の借用と走査は 147.874791 ms、UniFFI の複製と走査は 6,849.873958 ms。全回の和も照合した。これはこの生成コードと検証データの値で、製品のフレーム時間の予測ではない。10 万は検証の規模で、製品の上限ではない

配列は橋渡しの検証用の描画データである。UI-1 の可視範囲の問い合わせを実装したものではない。`Project` の内部の `Vec`、`Arc`、Rust のノートのメモリ表現を、そのまま Swift から読むこともしていない。

## 決定

命令と描画データを、cbindgen が生成する C ABI にそろえる。`EchodeBridge` が Swift の非同期関数、エラー、値、所有権へ包む。現時点の利用側は Swift だけである。大きい配列には C ABI が必要で、UniFFI を併用すると 2 つの境界と所有権の接続を保つことになる。通常の非同期処理に UniFFI を使えることは確認したが、生成物の修正なしで使える 1 つの境界を選ぶ。

契約は次のとおりである。

1. 命令は制御スレッドのキューへ送ってすぐ戻る。Rust の完了コールバックが、成功かエラーと、成功時の所有するスナップショットのハンドルを 1 回だけ返す。Swift は `CheckedContinuation` を 1 回だけ再開する。UI スレッドで `DocumentHost::perform` のようなブロックする処理を直接呼ばない
2. コールバックの context は Swift が保持して送り、完了時に取り戻す。Rust はそのポインタを読み書きせず、制御スレッドへ渡して返すだけである。入力が不正、キューが停止した場合も、完了を返して context を解放できるようにする。Rust はロックを保持したまま Swift を呼ばない。通知を UI へ渡すときは、小さい `Sendable` の値にし、`MainActor` へ明示的に渡す
3. スナップショットのハンドルは `Arc<Project>` と、不変の C レイアウトの描画用ビューを所有する。取得と retain は参照カウントだけを増やし、free はその参照だけを放す。Swift の final class が所有するハンドルを `deinit` で 1 回だけ解放する。借用する配列は `withUnsafeNotes` のような同期のクロージャーの間だけ読み、ポインタを外へ保存せず、その間に `await` しない。Rust の内部の型のレイアウトは ABI にしない
4. ポインタを持つ Swift の 2 つの final class に限り、理由を書いた `@unchecked Sendable` を置く。スナップショットは不変で、ドキュメントの変更は Rust のキューで直列化される。ポインタを持たない値と完了の context は、コンパイラが検査する `Sendable` にする。`@preconcurrency` と `nonisolated(unsafe)` を手書きの Swift に足して検査を回避しない
5. Swift Task がキューへ送る前にキャンセルされていれば、送らない。送った後のキャンセルは編集の取り消しを意味しない。完了の通知と解放は必ず行う。取り消すには別のアクションか Undo を送る。Rust Future を破棄しても、すでに送った命令と変更通知が残ることも Rust のテストで確かめた

メーター、プレイヘッド、CPU 負荷のアトミックな計測値は、これまでの共有メモリの方針を保つ。この橋渡しは制御と UI のためのもので、オーディオスレッドからは呼ばない。製品の `echode-ffi` と `EchodeBridge` は、アプリの骨組みでこの契約を実装する。検証コードは `spikes/bridge` に置き、製品のコードから参照しない。

C のヘッダ生成だけでは所有権や寿命は検査できない。完了の二重呼び出し、context の解放漏れ、借用の寿命を守る責任は手書きの層に残る。これらの契約を製品でもテストする。新しい C ABI は公開する前にエラーと panic の扱いも固める。今回の検証にはパッケージの保存、ジャーナル、アプリのウィンドウ、Metal、メーターは含まれていない。

## 再現と確認

`make spike-bridge` が、両方の生成器、Swift のコンパイル、実行、計測を行う。結果は `target/bridge/report.txt`、非同期コールバックのコンパイルエラーは `target/bridge/uniffi-async.log` に残る。既知の失敗は生成ファイルの `SendingClosureRisksDataRace` だけを受け入れる。他のコンパイルエラー、リンクエラー、実行の失敗はコマンドの失敗となる。上流が修正されたときは、非同期コールバックもコンパイルして実行する。

`make check-bridge` は、同じ Swift の検証を計測なしで行い、`make check` も呼び出す。Rust の所有権と操作列の検証も `make check` に含まれる。非同期コールバックの Rust 側と生成器は `cargo test -p echode-bridge-spike --all-features` と `cargo clippy -p echode-bridge-spike --all-targets --all-features -- -D warnings` で確認する。初回は通常の `cargo fetch --locked` で依存を取得する。生成時と再実行にはネットワークを使わない。

今回、`make spike-bridge`、Rust の全 feature のテストと clippy、生成器を含む依存のライセンス検査、`make check` は通った。`make check` は、実行環境の sandbox が既存の Unix ドメインソケットのテストを拒否したため、sandbox の外で再実行した。

## 製品へ引き継ぐ検査

2026-10-10 に、C ABI の採用と検査の強化を一緒に扱う提案をユーザーが承認した。ヘッダの生成、Swift の strict concurrency、通常の動作の検証だけを、安全性の確認の完了条件にはしない。

- 各 ABI に所有するハンドル、借用の範囲、null を許す条件、完了の実行場所と回数を書く。Swift のアプリ側へ生のポインタを広げない。`withUnsafeNotes` は呼び出し側が借用を守る unsafe API であり、ポインタの持ち出しを型で禁止したものではない
- 成功、空の入力、不正な UTF-8、キュー停止、処理途中の panic、送信後のキャンセルを検査する。停止した文書は新しい命令を拒否する。通常の終了は受け付け済みの命令を処理し、worker の異常終了は受け付け済みの命令すべてへ停止エラーを返す。context と continuation は一度だけ消費する。待ち続ける不具合は検査のドライバーが失敗として止める
- 文書、worker、スナップショット本体、C のハンドル、完了の context を文書ごとに数える。Swift の PendingCall も数える。文書を解放しても保持したビューが読め、最後の参照と worker がなくなったら、すべてがゼロへ戻ることを確認する。同じスナップショットのハンドルだけが漏れた場合も、本体の数だけで済ませずに検出する
- Rust と Swift の構造体のサイズ、配置、配列の stride を照合する。可視要素など呼び出し側が配列を渡す API では、容量ゼロ、ちょうど足りる場合、容量不足、書いた数と必要な数、数値の境界も検査する。配列の容量外を書かないことと、可視判定の内容が正しいことは別々に確認する。このスパイクではノートの ABI のサイズと配置を両言語で検査し、可視要素の API の検査はその実装へ引き継ぐ
- TSan は Rust、Rust の依存と標準ライブラリ、Swift に同じプロセスで検査を入れる。Rust は `-Zexternal-clangrt` を使い、静的ライブラリを Swift の最終リンクへ渡す。実行時ライブラリは Xcode のものを一つだけ使う。Rust の同期が TSan から見えるよう標準ライブラリも作り直す。ASan は後述の制約のため言語ごとに実行する。性能とオーディオの締め切りは、検査を入れないビルドで測る。既存の RTSan は別の規則を検査するため残す
- 検査を信用する前に、別の子プロセスにある Rust と Swift の意図的な解放後参照、または競合を両方検出できることを確認する。子の終了だけでは成功にせず、検査の診断と不具合を入れた場所を要求する。意図的な不具合は通常の橋渡しライブラリへリンクしない。検査の警告を広い抑制で消さない
- proptest は有効なハンドルの取得、retain、解放、読み取り、編集と不正な入力の操作列を作る。無作為なアドレスや、契約に反する二重 free を正常に処理することは要求しない。Miri は Rust で実装した C ABI と Rust のコールバックの所有権を検査し、Swift との実際の往復は別に検査する。Miri ではファイルへの失敗例の保存だけを止め、所有権や競合の検出、実行の隔離は保つ。通常の proptest は失敗例を保存する

スパイクの `fault-injection` feature は、処理を止める位置と変更前の panic を作る。待ち合わせは条件変数とキューへの送信の通知を使い、sleep による偶然の順序には頼らない。Rust の命令処理で unwind する panic を捕捉し、文書の受付を閉じ、現在と待機中の命令をエラーで完了する。C の生成関数で捕捉した生成失敗は null を返し、Swift はエラーにする。メモリ不足による abort、`panic=abort`、Swift の trap、無効なポインタは捕捉できるものとして扱わない。製品の DocumentHost と保存の異常処理を、このスパイクだけで検証したことにはしない。保存済みの変更の扱いも製品の契約で固める。

`make setup-bridge` が検査用の nightly-2026-10-09、rust-src、Miri と標準ライブラリの依存を用意する。製品の Rust 1.96.1 と既定の toolchain は変更しない。`make check-bridge-asan` と `make check-bridge-tsan` はそれぞれ `target/bridge/address` と `target/bridge/thread` に分離してビルドする。バージョンと実行のログもここに残る。ASan のリーク検出を前提にせず、上の保持数で解放を検査する。`make check-bridge-miri` は C ABI の Rust テストを seed 1、17、42 で実行する。操作列の件数は通常 64、Miri は 4 で、検査の予算であり製品の上限ではない。

ASan の接続には制約がある。この nightly の LLVM は 23.1.3、Xcode の Clang は 21.0.0 で、Rust は `__asan_version_mismatch_check_v8`、Xcode は `__asan_version_mismatch_check_apple_clang_2100` を要求する。同じプロセスへそのままリンクできることは確認できず、バージョン確認を消したり、偽の関数を追加して通すことはしない。Rust の C ABI と Rust のコールバックは、Rust の実行時ライブラリで標準ライブラリと依存も含めて検査する。Swift の実際の C ABI の往復は、Xcode の実行時ライブラリで Swift に検査を入れ、Rust の通常の静的ライブラリにつなぐ。両言語の意図的な解放後参照もそれぞれの実行時ライブラリで検出する。これは両側を一つのプロセスで同時に ASan 検査した証拠にはならない。製品の橋渡しを公開する前にも、ツールチェーンの接続とこの検査範囲を確認する。

CI の通常の検査は Swift の検査を含む。ASan、TSan、Miri は別ジョブにし、失敗もログを保存する。macOS 27 と Xcode 27 の arm64 環境には GitHub の `xcode-27` ラベルを使う。公開プレビューなので、実行環境の提供状況とバージョンは更新時に確認する。

追加した検査はこの Mac で通った。`make check`、生成器を含む全 feature の clippy とライセンス検査、`make check-bridge-asan`、`make check-bridge-tsan`、`make check-bridge-miri` を実行した。Rust の ASan は 11 件、Miri は 6 件を各 seed で実行した。両言語の意図的な不具合の診断も確認した。Swift の保持数の確認は、先行する完了の終了を待ち、検査の Task 自身が結果のハンドルを保持し続けないようにしている。検査のドライバーは期限を超えると Cargo の子のテストも含めて終了し、その経路も別の子プロセスで確認した。CI の設定の構文は確認したが、GitHub 上での実行はまだ確認していない。

## 参照

- [UniFFI の Swift 6 対応の説明](https://mozilla.github.io/uniffi-rs/latest/swift/overview.html)
- [UniFFI 0.32.2 の変更履歴](https://github.com/mozilla/uniffi-rs/blob/v0.32.2/CHANGELOG.md)
- [非同期コールバックの strict concurrency エラー（上流 #2929）](https://github.com/mozilla/uniffi-rs/issues/2929)
- [cbindgen 0.29.4 のヘッダ生成と設定](https://github.com/mozilla/cbindgen/blob/v0.29.4/docs.md)
- [Rust の sanitizer と他言語との接続](https://doc.rust-lang.org/unstable-book/compiler-flags/sanitizer.html)
- [Miri の検査対象と制約](https://github.com/rust-lang/miri)
- [GitHub の Xcode 27 / macOS 27 の実行環境](https://github.blog/changelog/2026-09-10-xcode-27-runner-image-now-runs-on-macos-27/)
