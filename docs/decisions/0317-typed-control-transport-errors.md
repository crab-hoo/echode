# ADR 0317: 記述子の規則の違反と送信の拒否の原因を保持する

日付: 2026-11-11
状態: 採用
関連: M1（DEV-7・SPG-11・XCA-16）、[ADR 0316](0316-typed-parameter-payload-errors.md)、[ADR 0288](0288-received-descriptors-are-all-owned.md)、[ADR 0042](0042-plugin-ipc.md)

## 問題と再現

開始時の HEAD は `4309158`。
`control.rs` に残る文だけの原因は、受信側の記述子の規則の違反 3 種と、送信側の引数の拒否 2 種だった。

- 受信: 補助データの切詰め、1 回の読込みに 2 個以上の記述子、前の記述子が使われる前の次の記述子。
  理由は `&'static str` で持ち、その後の受信も同じ文を返していた。何個届いたかは型で読めなかった。
- 送信: `ControlMessage::Remap` を `send_tagged` に渡すと、`InvalidInput` の `remap carries a file descriptor` で拒否した。
  本体が 64 MiB を超える枠は `InvalidData` の `control message is longer than 64 MiB` で拒否し、種類と長さを型で読めなかった。

製品の `Socket` で回帰を先に追加した。修正前は 5 つすべてが失敗した。
記述子の 2 つは `a broken descriptor rule must retain its typed cause`、64 MiB の 2 つは `a refused frame must retain its typed size` で失敗した。
`send_tagged` の remap は `remap carries a file descriptor` の `Err` で失敗した。
種類と表示文の照合は修正前も通るので、型の欠落だけを示す。
期待終了コード 101 と固有のアサーションの照合は `target/focused-checks/control-transport-red/` に残す。

## 比べた案と決定

1. 定数の文を残す。何個の記述子が届いたか、どの種類の枠がどれだけ長かったかが同じ文へ消えるので採らない。
2. `ControlDecodeError` に変種を足す。あちらはバイト列の構造で、記述子は補助データ、送信の拒否は自分の引数である。
   受信の違反だけはソケットを使えなくする性質も違うので、一つの型に混ぜない。
3. **受信の違反を `DescriptorRuleError`、送信の拒否を `ControlSendError` に分ける。remap の拒否は型にせず、なくす。**

3 を採る。

- `DescriptorRuleError` は `ControlTruncated { received }`・`ExtraDescriptors { count }`・`UnconsumedDescriptor` である。
  判定は純粋な `descriptor_rule` の一か所にまとめ、従来の順（切詰め → 2 個以上 → 未使用の記述子）を保つ。
  ソケットは文ではなくこの値を保持し、後の受信も同じ原因を返す。全記述子を閉じる処理は変えない（ADR 0288）。
- `ControlSendError::TooLong` は枠の種類と長さのフィールドの値（種類・ID・本体）、上限を持つ。種類は `InvalidData` のままである。
- `send_tagged` は `Remap` の記述子を枠と一緒に渡す。カーネルが複製するので、メッセージは自分の写しを持ったままである。
  `send_remap_tagged` はこの経路を使い、渡された記述子を送信後に閉じる。拒否の経路がなくなり、fuzz の往復も分岐が要らなくなった。

時間切れ・切断・書込み 0 バイトは `TimedOut`・`UnexpectedEof`・`WriteZero` が原因そのもので、文は補足なので残す。
表示文・受理する形式・複数の違反がある時に先に返る原因は変えない。音声処理・期限・保存・C ABI は変えず、性能改善は主張しない。

### 文字列リテラルの再導入の検査

`check_typed_errors.py` に `CONSTANT_TEXT_CRATES` を足し、`echode-sandbox` で I/O エラーに文字列リテラルだけを渡すことを拒否する。
許すのは種類が原因そのものの `TimedOut`・`UnexpectedEof`・`WriteZero` だけである。`io::Error::other("...")` も拒否する。
構文の検査なので、変数に入れた `&'static str` は見つけない。修正前の `control.rs` では、remap と 64 MiB の 2 か所を見つける。
`echode-history` には `NotFound` 2 か所と `InvalidInput` 2 か所のリテラルがあり、まだ対象に入れない。

## 回帰と基盤

- `control_rights_tests.rs` は、実ソケットで 2・3・254 個の記述子（remap と通知）と、未使用の記述子の後の記述子を型・従来の文面・後の受信で照合する。
  Darwin は受信バッファを超える記述子の送信を拒否するので、切詰めは実ソケットで作れない。判定の順と数は `descriptor_rule` の表で固定する。
- `control_send_tests.rs` は、上限ちょうどの本体の枠と、1 バイト超えの拒否を型と文面で照合し、拒否した枠が何も書かないことを確かめる。
  `send_tagged` の remap が記述子付きで届き、送り手の写しが開いたままであることも検査する。
- `session_operations_tests.rs` は、上限を超える状態の `restore_state` が `SessionError::Io` に同じ原因を残し、何も送らず、次の要求が通ることを検査する。
- `scripts/tests/test_typed_errors.py` に、リテラルの検出と、許す種類・他の crate を対象外にすることの表を足す。
- 製品契約 `plugin-control-descriptors` の反例（拒否を保つ行を消す）は、保持する値の新しい名前 `rule` に合わせた。
  変えた反例で契約が `the rejected remap is not accepted later: Ok(None)` で失敗することを手で確かめた。
- 制御ソケットの fuzz は記述子を 0〜3 個付けて送り、既存の `control_inputs_replay` が通す。Makefile と検査選択は増やさない。
- 領域別設計、コードの所在、診断手順、監査の状態を更新する。利用者の制約・実機の確認場面は増えず、status.md とユーザー確認の一覧は変えない。

## 調査で分かった別の問題

ヘルパーは保存の応答を `send_tagged` で送り、失敗すると制御ループを止める（`threads.rs` の `handle_control`）。
プラグインの状態が 64 MiB − 12 バイトを超えると `ControlSendError::TooLong` で送れず、そのヘルパーは止まる。
ADR 0042 は 64 MiB を「状態の上限ではない」としているが、実際には上限として働く。製品の経路では再現しておらず、64 MiB を超える状態を返すテスト用のプラグインもない。
音切れと保存の失敗に当たるので、統合表の堅牢性の行 R8 として登録し、M1 の続きより先に扱う（ADR 0276 の順）。

## 残り

M1 全体の完了とはしない。`echode-sandbox` の制御ソケットの文だけの原因は、種類が原因そのものの 3 つを除いてなくなった。
次の M1 は `echode-history` のリテラル 4 か所を型にし、`CONSTANT_TEXT_CRATES` を型付きの全 crate へ広げることである。
検査結果と各検査の見送り理由は `target/verification/report.md` を正本とする。
