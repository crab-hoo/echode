# ADR 0313: 起動拒否の不正なペイロードの原因を保持する

日付: 2026-11-10
状態: 採用
関連: [ADR 0308](0308-typed-helper-launch-failures.md)、[ADR 0312](0312-typed-control-text-errors.md)、XCA-16 / M1

## 問題と再現

開始時の HEAD は `cb08aa9`。
`LaunchFailure::decode` は、不足したメタデータ・未知の種類・不正な errno のフラグ・診断文の不正 UTF-8 をすべて `None` にしていた。
`read_refusal` は定数の文だけの `io::Error` へ畳み、元の原因を読めなかった。

製品の `Socket::recv_tagged` へ起動拒否の生フレームを送る回帰を先に追加した。
修正前は `invalid launch diagnostic must retain the UTF-8 decoding cause` で失敗した。
期待終了コード 101 と固有のアサーションの照合は `target/focused-checks/launch-decode-red/` に残す。

## 比べた案と決定

1. `Option` と定数の文を残す。種類・位置・長さを復元できないため採らない。
2. UTF-8 だけ型にする。メタデータの切詰めと不正なコードは識別できないままなので採らない。
3. **読込みで `LaunchDecodeError` を返し、`io::Error` の内側にも同じ型を保持する。**

3 を採る。`Truncated` は `LaunchField`・必要なバイト数・残ったバイト数を持つ。
種類、NUL の位置、errno の有無と値、プラグイン側とホスト側の版をフィールドで分ける。
未知の種類と不正な errno のフラグは受けた値を持ち、`Diagnostic` は元の `Utf8Error` を持つ。
診断文の位置はその文字列の先頭を基準とし、不正な診断文のバイト列は保存しない。

表示文 `launch failure is malformed`、`InvalidData`、Socket の戻り値、通信の形式と受理する入力は変えない。
`io::Error::get_ref` から型を読み、診断文の失敗では `Error::source` から元の UTF-8 の原因も読める。
外側が `SessionError::Io` になっても保持する。性能改善は主張しない。

## 回帰と基盤

- `launch_failure_tests.rs` は全種類の往復、全メタデータの切詰めと、不正な診断文・コードの拒否、受理した符号化の正規性を維持する。
- `control_launch_tests.rs` はポート・状態・パラメーター・窓の拒否を実ソケットへ通す。
  全フィールドの切詰め、全未知コード・不正フラグ、不正・未完の UTF-8 を、原因と従来の表示文で照合する。
- 空・Unicode・埋込み NUL の診断文と、符号の境界を含むメタデータは変更せず受理する。
- `session_operations_tests.rs` は公開するパラメーター操作で、不正なヘルパーの応答が元の型を保持することを検査する。
- 制御ソケットの fuzz の種に、起動拒否の不正・未完の診断文を追加する。

通常の Rust 検査と既存の `control_inputs_replay` が実行するため、Makefile と検査選択は増やさない。
領域別設計、コードの所在、診断手順、監査の状態を更新する。
音声処理・期限・保存・C ABI は変わらず、新しい実画面・実機の確認場面はない。

## 残り

M1 全体の完了とはしない。
次は `read_plugin_failure` と、`read_refusal` の残る構造の拒否で、種類・フィールド・長さの原因を定数の文にしている境界である。
検査結果と各検査の見送り理由は `target/verification/report.md` を正本とする。
