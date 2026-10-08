# 0030. アクションの ID は登録簿の 1 つだけにする

- 日付: 2026-10-08
- 状態: 採用
- 項目: API-1

## 背景

モデルを変える操作は、依頼の `Command`、確定した `Action`、ジャーナルの `ActionRecord`、API の `Requested`、登録簿 `SPECS` に分かれている。`spec()` は `&SPECS[6]` のように添字で引く。`actions.list` は `track.add` を返し、`actions.perform` とジャーナルの `type` は `add_track` を使う。API の結果は、ジャーナルの文字列を作ってから JSON に読み戻している。

依頼と確定はフィールドが違う。確定は採番した ID を持ち、ゲインは `Eq` のためにビットで持つ。依頼のゲインは `f32` である。

## 調査

引数の JSON Schema は `schemars` 1.2.2（MIT）が、監査の挙げる選択肢である。`cargo deny` の許可するライセンスに MIT は入っている。

## 決定

- 操作の ID は登録簿の文字列だけにする。今の 12 個は `track.add`、`track.add_child`、`track.set_parent`、`project.set_meter`、`project.set_tempo`、`note.insert`、`note.delete`、`clip.resize`、`clip.link`、`clip.set_gain`、`clip.set_fades`、`param.set` のままである。ジャーナルと `actions.perform` の `type` もこの文字列にする。`add_track` のような古い名前は読まない。ジャーナルの移行はしない
- 定義は `echode-actions` の `define_actions!` に 1 つのリストで書く。各項目は、ID、日英の名前と説明、依頼のフィールド、確定のフィールド、依頼から確定を作る式、モデルへ当てる式である。マクロが `Command`、`Action`、serde の `type`、`catalog()`、`spec()` を作る。`ActionRecord` と `Requested` は消す。登録簿の添字は使わない
- 操作を足すときは、このリストに 1 項目足す。プロパティテストの操作の生成は、新しい variant を網羅しないとコンパイルが止まるので、そこも直す。ジャーナルや API の型を別に足す場所は残さない
- `Id` はハイフン付きの文字列として serde で読み書きする。`Tick` は整数、拍子とテンポは `numerator` と `denominator` のまま、ノートの依頼は今のとおり平らな `pitch`、`velocity`、`start`、`length` のままにする。確定のゲインとパラメータは、今の `gain_bits` と `value_bits` のままにする。`content` が無い古い行は、今どおり読める
- 依頼の JSON Schema は `schemars` 1.2.2 が各項目から作る。`actions.list` の各要素は、今の ID と名前と説明に、その schema を足す。`actions.perform` は依頼を serde で読み、結果は確定した `Action` を serde で書く。ジャーナルの文字列を経由しない
- 拍子が tick に割り切れないとき、テンポが範囲外のとき、ID が読めないときは、その文をパラメータのエラーにする。未知のフィールドと、型の違う値は、今の「parameters do not match an action」にする
- ADR 0020 の「ジャーナルと同じ `type` 名」は、この登録簿の ID を指すように改める

## 結果

`make check` が通った。`actions.list` の `track.add` で `actions.perform` を呼ぶと、結果の `type` も `track.add` になる。一覧の schema には依頼のフィールドが入る。ノートの schema には `pitch` と `start` がある。ジャーナルは `track.add` と `note.insert` を往復し、`add_track` は拒否する。拍子が tick に割り切れないとき、テンポが範囲外のとき、ID が読めないときは、その文がパラメータのエラーになる。未知のフィールドも、古い `type` 名も拒否する。
