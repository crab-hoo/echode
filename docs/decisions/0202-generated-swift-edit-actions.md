# 0202. アクション登録簿から Swift の編集型を生成する

- 日付: 2026-10-30
- 状態: 採用（生成器・橋渡し・ピアノロールを移行、他領域の controller 編集は後続）
- 対象: [構造監査](../reviews/2026-10-30-structure-audit.md) SPG-03

## 背景と今回の範囲

Rust の `define_actions!` が命令・登録簿・JSON Schema を生成する一方、Swift の編集要求は辞書で組み立てている。
キーの改名や型の変更を Swift のコンパイル時に検出できない。
不具合の再現ではなく、この重複をなくすための段階的な構造整理である。
最初の受入単位で生成器・生成物・型付きの製品橋渡しを導入した。後続の単位でピアノロールを移行した。

## 決定

- `echode-actions` の `swift_actions` example が `catalog()` と各アクションの公開スキーマを読み、
  `EchodeBridge.EditAction: Encodable, Sendable` を生成する。操作の一覧や項目を生成器へ再定義しない。
- case 名はスキーマの title、引数は必須項目の順と残りの項目から導く。
  Swift の snake_case → camelCase と JSON の元のキーを分け、キーワードは backtick で保護する。
  衝突する名前や未知のスキーマ構造・format は生成エラーにする。
- 文字列・真偽値・整数・Float と nullable の現在のスキーマを扱う。
  ID の構文、ノートの範囲、参照の存在、テンポ・拍子の妥当性は引き続き Rust が検証する。
  公開スキーマ上の ID は `String`、format のない整数は `Int64` とし、
  `u8` / `u16` / `u32` / `u64` と `f32` は対応する Swift の型を使う。
  既存スキーマを変更して UUID・Tick の意味を推測する生成器にはしない。
- optional の nil は明示的な JSON null にする。親・保存済み状態の解除と、末尾へのデバイス移動を保つ。
  時刻・フレーム数を Double へ変換せず、Float は `JSONEncoder` で符号化し、非有限値は送信前に拒否する。
- `Document.perform(EditAction)` と `Document.perform([EditAction])` は、
  既存の raw JSON の submit と同じ文書ワーカーへ渡す。
  配列の単一 Undo・巻戻し・no-op・送信前の取消と、送信後に編集を取り消さない契約は変えない。
- 生成物を追跡する。`make swift-actions` が再生成し、`make check-swift-actions` は変更せず鮮度だけを検査する。
  後者を通常の `make check` に組み込む。生成器は製品アプリへ同梱しない。

## 回帰と診断

- Rust の example テストは生成物の一致、全登録操作を覆う共有 wire fixture の `Command` 往復、
  未対応のスキーマ・名前の衝突・古い生成物の拒否を確認する。
- Swift の `EditActionTests` は全 case を独立した共有 fixture と比較する。
  大きい整数の表記、nullable の両側、Float の極小・極大と非有限値も確認する。
- 必須契約 `typed-edit-bridge` は実際の文書ワーカーへ単一／一括要求を送り、
  履歴・一括失敗・符号化失敗・取消・no-op とパッケージの再オープンを確認する。
  生成物のベロシティ送信を誤らせる負の対照も持つ。
- `is stale; run make swift-actions` は製品モデルの拒否ではなく生成物の不一致である。
  元の登録簿・モデル・スキーマを確認し、再生成して差分を確認する。
  未対応のスキーマは手で生成物を修正せず、生成器と両言語の回帰を同時に拡張する。
- ログや ABI の新しい出力先はなく、既存の診断収集を維持する。

## 後続: ピアノロールの controller を移行する

ノートの挿入・単一編集・削除・ベロシティと、複数移動・複製・伸縮・キー操作を生成型へ移した。
`PianoMoveChange`・`PianoResizeChange`・`PianoStep`・`PianoVelocityChange` は `[EditAction]?` を返す。
controller は一要素の要求も配列へそろえ、型付きの `Document.perform([EditAction])` に送る。
MIDI の `Int` は既存の範囲・幾何・元の値の照合後にだけ `UInt8` へ変換し、不正入力を丸めて送らない。

`performActions` と未移行の辞書の adapter は、型を保つ generic の `applyEdit` を共有する。
編集順序・文書照合・no-op 解決・履歴・入力長とベロシティの記憶を二重実装しない。
対象と no-op は順番が来てから解決し、解決できない要求で入力記憶を更新しない。
型付きの経路は `Any` や `JSONSerialization` を通さない。

`TypedPianoEditTests` は `2^53` を超える整数時刻と二つの対象、単一 Undo / Redo、古い元の値の拒否を検査する。
MIDI の境界外と旧文書を拒否した時の履歴・入力記憶、連続キーの実行順序、保存再読込も確認する。
必須契約 `typed-piano-controller` は、複数要求の先頭を落とす反例を拒否する。
既存のピアノロールの操作・リンク・取消の契約は対象と期限を維持する。
検査基盤は移行済みの組立てに辞書を戻さないことと、新しい契約の選択・反例の一意性も照合する。

## 残る範囲

トラック・デバイス・ミキサー・クリップ移動・音声の controller 編集には辞書が残る。
特に新規トラックへの音声ドロップは、一括編集内の結果参照を使うため、生成型の通常の ID とは別に扱う必要がある。
これらの移行と、型のない controller の編集入口の除去は後続とする。
raw JSON の `Document.perform(String)` は互換入口として保持する。
ID と時刻を UUID / Tick の値型として生成すること、文書スコープの集約（SPG-02）は今回の完了には含めない。
音声処理・描画・保存形式は変えず、実機・聴感の新しい合格を主張しない。
