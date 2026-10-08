# 0034. API で複数の操作を 1 回の Undo にする

- 日付: 2026-10-09
- 状態: 採用
- 項目: API-2

## 背景

ARCHITECTURE §15 の自然言語コマンドは、複数のアクションからなる実行計画を、1 回の Undo で戻せる単位として実行する。MCP から操作するエージェントも同じことをする。今の JSON-RPC は `actions.perform` が 1 操作だけで、Undo と Redo も、クライアントがサーバーのプロトコル版を確かめる手段も無い（[ADR 0020](0020-json-rpc.md)）。

操作の束ね方は、モデル側では `Session::transaction` がすでにある。途中で失敗すると、プロジェクトも Undo の積みも操作前に戻る（[ADR 0008](0008-actions-undo-journal.md)）。ドキュメントを変えるのは `DocumentHost` のスレッドだけで、成功した操作はジャーナルを 1 行書いてから結果を返す（[ADR 0032](0032-document-host.md)、[ADR 0033](0033-journal-log.md)）。

依頼の時点では、サーバーが採番する ID はまだ無い。配列を渡すだけでは、作ったトラックへノートを置く計画を 1 回の Undo にできない。失敗した要素の位置がエラーに無いと、クライアントはどの操作が拒否されたか決められない。`Session::undo` と `Session::redo` は `()` しか返さないので、追記する ChangeSet を戻り値からは取れない。Undo の行は ChangeSet だけで再生し、`edit` だけが `actions` を持つ（ADR 0033）。Redo の行に操作の列を書くと、その形と食い違う。

## 決定

- プロトコルの版は `protocol.version` が返す。結果は `{ "version": 1 }` である。版 1 は、今の `actions.list`、`actions.perform`、`project.summary` に、この ADR のメソッドを足したものである。意味を変えるときは版を上げる。アプリの版とは別である
- 複数の操作は `actions.transaction` の 1 回で行う。パラメータは `{ "actions": [ ... ] }` で、各要素は `actions.perform` と同じ依頼である。`type` は登録簿の ID のままにする（[ADR 0030](0030-action-catalog.md)）。begin と commit は置かない
- 配列の中では、まだ無い ID の代わりに `{ "ref": <0 からの番号>, "field": "<確定した操作のフィールド名>" }` と書く。番号はその要素より前の操作だけを指す。フィールドは、その操作の結果 JSON にある ID の名前である。例は、`track.add` の `id` や `gain` である。解決してから、普通の依頼として読む。`actions.perform` はこの参照を受けない
- 配列の順に当てる。全部成功したら、セッションのトランザクションと同じく 1 つの Undo 単位になる。ジャーナルも 1 行で、種類は `edit`、中身はその操作の列と、まとめた ChangeSet である。連番は 1 つ進み、自動保存もその連番で 1 回頼む。結果は、確定した `Action` の配列である
- 空の配列は成功し、結果は `[]` である。Undo もジャーナルも積まない
- どれか 1 つが失敗したら、全部を操作前に戻し、ジャーナルは書かない。エラーの `data.index` は、0 から数えたその要素の番号である。モデルが拒否したときは `-32000` で、その文を返す。依頼の形が違うとき、参照が範囲外のとき、フィールドが ID でないときは `-32602` で、要素なら今の文（「parameters do not match an action」、拍子、テンポ、ID）か、参照の文（「action reference is out of range」、「action reference is not an index」、「action reference is missing a field」、「action reference field is missing」）を返す。`actions` 自体が配列でないときは番号を付けず、「parameters do not match a transaction」にする
- `edit.undo` と `edit.redo` は引数を持たない。成功すると結果は `{}` である。戻すものが無いときは `-32000` で、「nothing to undo」または「nothing to redo」を返す
- `Session::undo` と `Session::redo` は、その行に書く ChangeSet を返す。これは取り消す、またはやり直す操作の順方向の差分である。Undo の再生は、今どおりその逆を当てる。Redo の再生はそのまま当てる
- Undo と Redo のジャーナルは 1 行ずつである。種類は `undo` と `redo`、`actions` は空、`changes` は上の ChangeSet である。空の `actions` は、今のエンコーダと同じくファイルには書かない。欠けたキーは空として読む。どちらの行も、書いてから結果を返す。自動保存もその連番で 1 回頼む
- ADR 0020 のメソッド一覧と、ARCHITECTURE §15 のメソッドの文は、このメソッドを含む一覧に改める

## 結果

`make check` が通った。トラックの追加、その ID へのノート、そのゲインの変更を `actions.transaction` の 1 回で行い、`edit.undo` の 1 回で 3 つとも戻る。やり直しの行も `actions` は空である。2 番目の要素が拒否されると `data.index` は 1 で、プロジェクトは操作前のままである。`protocol.version` は 1 を返す。
