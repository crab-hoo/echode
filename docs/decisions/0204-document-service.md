# 0204. 文書の手番の調停を FFI から文書サービスの crate へ移す

- 日付: 2026-10-31
- 状態: 採用
- 対象: [構造監査](../reviews/2026-10-30-structure-audit.md) SPG-05
- 関連: [ADR 0136](0136-output-opening-off-worker.md)、[ADR 0131](0131-device-view-and-responsive-parameters.md)、[ADR 0203](0203-document-session.md)

## 背景と再現

開始時の HEAD は `cf190e7`。
C ABI の crate の `queue.rs` の `worker`（約 240 行）が、履歴・音声セッション・デバイスレーン・プラグイン保存の取込み・出力待ちの要求を調停していた。
`catch_unwind` と完了通知の分岐が 3 回、文書変更時の破棄が 2 回、ほぼ同じ形で書かれていた。
読解で疑った 2 点を、修正前のコードで `completion_tests.rs` を実行して再現した。

- **投入済みの完了の消失**: `Job` は破棄されても完了しない。
  保護のない手番（保存の取込みと `audio.update`、メーターと位置の公開、起動時）の panic でワーカーが終わると、キューの要求の完了が呼ばれない。
  Swift の継続は戻らないまま残る。起動時の設定に panic を注入し、`a queued command must not lose its completion: Timeout` で失敗した。
  製品で panic が起きる具体的な入力は確認していない。保護の有無が手番ごとに違うこと自体を構造の欠陥として扱う。
- **自スレッドの join**: デバイス要求の完了はレーンのスレッドで走る。
  そのコールバックから `echode_document_shutdown` か `echode_document_close` を呼ぶと、レーンがワーカーを、ワーカーがレーンを join する。
  macOS が循環を拒み、16 回中 16 回ワーカーが後片付けの途中で `failed to join thread: Resource deadlock avoided` で異常終了した。
  C の入口側が後に join すれば、`extern "C"` の中の panic で abort する。この順序は競合で決まり、abort 自体は観測していない。
  現在の Swift の完了関数は継続を戻すだけなので、製品の画面操作はこの経路を通らない。C ABI の約束の問題である。

## 決定

- **`echode-document` crate**: 文書の手番を C ABI の外へ移す。
  `worker.rs` が 1 手番ずつ実行し、`saves.rs` が保存の取込みの判定、`lane.rs` と `device.rs` がデバイス要求、`activity.rs` と `live.rs` がワーカーを待たない読取りを持つ。
- **`DocumentService<V>`**: `submit(Operation, Ticket<V>)` で投入し、戻りを待たない。文書を開くごとに 1 つ持つ。
- **`Presenter`**: 完了に載せるものをサービスの外で決める。
  サービスは `Publication`（プロジェクトと音声・履歴・デバイスの JSON）を渡し、アレンジの索引も波形も知らない。
  FFI の `SnapshotPresenter` が前回の `Snapshot` を持ち、変わらないプロジェクトでは索引と波形を再利用する。
- **`Ticket<V>`**: 完了は 1 回だけ走る。破棄すれば `document thread stopped` で完了する。
  panic の巻戻しで落ちる要求も完了するので、完了の消失を箇所ごとの処理に頼らない。
- **保護付き実行の 1 か所**: `guarded` が panic を `document thread panicked` に変えて受付を止める。
  文書を変え得る手番は `turn` を通り、変更時に保留中の保存と編集中の印を消す。
  保存の取込みと公開も保護の内側に入れた。停止後も再生位置とメーターの公開は続け、公開自体が panic したら消す。
- **終了の順序**: フィールドの破棄順で固定する（セッション → レーンの join → 読み手への通知 → 履歴）。
  ワーカーが最初の手番の前に死んでも、活動の読み手は `closed` を受け取る。
- **完了からの終了**: レーンのスレッドからの `shutdown` も、ワーカー上と同じく join せずに戻る。所有者が後で join するか、切り離す。
- **FFI**: `document.rs` が C のコールバックを `Ticket` へ変え、`snapshot.rs` が表示用の問い合わせを持つ。
  C の関数・構造体・完了の順序と、JSON の報告の形は変えない。生成ヘッダの差は `Document` のコメントだけである。
- **スレッド名**: ワーカーを `echode-ffi` から `echode-worker` へ変える（`echode-document` は既に履歴のスレッドが使う）。

## 監査の案との違い

- `handle(Operation) -> Outcome` の同期の戻り値にはしない。デバイス要求はレーンで後から完了するので、完了の受け口を `Ticket` にした。
- CLI と MCP は移行しない。`echode serve` は音声セッションを持たないので調停の差が今は生じない。
  GUI アプリの MCP 入口（[Phase 2](../phase2.md)）は、別の `DocumentHost` を開かずこのサービスへ投入できる。
- 文字列のエラーは保つ（SPG-11）。`PluginControl` の要求の集約も行わない（SPG-06）。

## 回帰と基盤の保守

- `document-worker-completion` は C のコールバックで完了の消失がないことを確認する。`Ticket` の破棄時の完了を外す反例を拒否する。
- `document-lane-completion` はレーンの完了からの終了で join の循環がないことを確認する。レーンのスレッドの照合を外す反例を拒否する。
  反例の失敗は通常 `refused_joins` の計数で出る。C の入口側が後になると abort になるため、回数を 4 回に抑えた。
- `service_tests.rs` は FFI なしの `Presenter` で手番・panic 後の拒否・起動時の死を確認し、`saves.rs` は取込みの判定を単体で確認する。
- 既存の `output-opening-worker`、`queued-stop`、`folder-projection-ownership` は新しい場所の同じ反例で確認する。
- 検査の選択は `crates/echode-document/**` を `echode-ffi` と同じ領域に加え、ASan のライブラリ回帰へこの crate を足した。
  `test_document_turns_stay_out_of_the_c_abi_crate` は、FFI の製品コードに `DocumentHost`・`catch_unwind`・`Session` が戻らないことを確認する。
- この作業の検査で、前回の `closingRemovesTheDocumentAndItsContentsTogether` が ASan の全件実行で一度失敗した。
  ポーリングを遅らせると同じ行で再現し、製品の `busy` の挙動ではなくテストの順序依存と分かったので、終了をゲートで止める形へ直した。
- `make check` の全件実行で `recovery_tests` がワーカーの `send` の途中で abort した。
  テストがスタックの `Sender` をコンテキストに渡し、受信後に関数を抜けて解放していた。製品の Swift は所有を渡すので該当しない。
  同じ形の `recovery_tests`・`audit_pro14_close_*`・`completion_tests` を、コールバックが所有する形へ揃えた。
- 新しい失敗の見分け方を [デバッグ手順](../debugging.md) に足した。ログ形式と出力先は変わらないので、`doctor` と `debug-report` の収集範囲は変更不要である。
- 新しいアクション・保存形式・DSP・外部依存は加えない。RT の経路と音を作る処理は変わらないので、ブロック長回帰は追加しない。
