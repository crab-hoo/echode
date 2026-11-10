# ADR 0312: 制御メッセージの不正な UTF-8 の原因を保持する

- 日付: 2026-11-10
- 状態: 採用
- 対象: M1（DEV-7・SPG-11・XCA-16）、`echode-sandbox`
- 前提: [ADR 0311](0311-typed-shared-memory-errors.md) の残り

## 問題と再現

開始時の HEAD は `1c75b1f`。
窓名、パラメーターの文字入力、名前・モジュール・値の文字列の読込みは、
`String::from_utf8` の原因を捨て、定数の文だけの `io::Error` を返していた。
拒否と表示文は正しかったが、不正な位置と、不正なバイト列か未完の列かを型で読めなかった。

製品の `Socket::recv_tagged` へ生のフレームを送る回帰を先に足した。
修正前は `invalid control text must retain the UTF-8 decoding cause` で失敗した。
期待終了コード 101 と固有のアサーションの照合は `target/focused-checks/control-text-red/` に残す。

## 比べた案と決定

1. 定数の文のままにする。診断文の解析では元の位置や列の長さを復元できないので採らない。
2. `Utf8Error` を直接 `io::Error` へ入れる。原因は保持できるが、既存の表示文と窓名・パラメーターの区別が失われる。
3. **公開する `ControlTextError` で文脈と `Utf8Error` を持ち、`io::Error` の内側へ入れる。**

3 を採る。`WindowTitle` と `Parameter` の 2 変種にし、既存の診断の分類に合わせる。
パラメーターの各フィールドの位置や ID を新たに複写する状態は持たない。
`io::ErrorKind::InvalidData`、Socket の戻り値、表示文、通信の形式と拒否する入力は変えない。
`io::Error::get_ref` から `ControlTextError`、その `Error::source` から元の `Utf8Error` を読める。
`SessionError::Io` への変換後も内側の型を保持する。

共通の `control_text.rs::read_text` が借りたバイト列を検証し、成功した時だけ文字列を作る。
`valid_up_to` と `error_len` はその文字列の先頭からの値で、フレーム全体からの位置ではない。
不正な入力バイトは原因へ保存しない。性能改善は主張しない。

## 回帰と基盤

- `control_text_tests.rs` は窓名、文字入力、パラメーターの名前・モジュール・表示値、適用後の値、通知の値を実ソケットへ通す。
- 各経路で不正なバイトと未完の UTF-8 を検査し、元の原因、変種、表示文を照合する。
- 空文字、Unicode、埋込み NUL は従来どおり受理し、正規化しない。
- 構造の切詰めは UTF-8 の失敗に分類しない。
- `session_operations_tests.rs` は公開する `PluginControl::parameters` から、壊れたヘルパーの応答の原因が届くことを照合する。
- 制御ソケットの fuzz の種に、不正・未完の文字列を持つ要求・応答・通知を追加する。入力形式は変えない。

通常の Rust 検査と `control_inputs_replay` が回帰と種を実行するため、Makefile と検査選択は増やさない。
既存の静的検査は原因を捨てるすべての構文を証明するものではなく、今回の再導入は製品経路の回帰で防ぐ。
領域別設計、コードの所在、診断手順と監査の状態を更新する。
保存・C ABI・音の処理・実機の期限は変わらず、新しいユーザー確認の場面もない。
実機の計測・聴感の新しい合格は主張しない。

## 残り

M1 全体の完了とはしない。
次は `LaunchFailure::decode` で、不正なメタデータと診断文の不正 UTF-8 が同じ `None` になり、
`read_refusal` で定数の文へ畳まれる境界を扱う。
検査結果と検査ごとの見送り理由は `target/verification/report.md` を正本とする。
