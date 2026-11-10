# ADR 0309: ヘルパーの無効な ID と内部の応答不一致を型で送る

日付: 2026-11-10
状態: 採用
関連: M1（DEV-7・SPG-11・XCA-16）、ADR 0293・0295・0308

## 問題と再現

開始時の HEAD は `15c89f4`。
`apply_params` は予約済みの CLAP ID を `Refusal::Other("invalid parameter change")` にし、
メインスレッドの応答の不一致も文だけにしていた。
ホストへ届く前に、無効な ID と期待・実際の応答の種類が失われていた。

製品の `PluginControl::set_parameters` と実ヘルパーを通す回帰を先に足した。
同梱のパラメーター用 CLAP に、通常の値の変更と `u32::MAX` の ID を一括で送ると、
`Refused(Other("invalid parameter change"))` となり、種類のアサーションで失敗した。
期待終了コード 101 と回帰固有の文を照合した記録は `target/focused-checks/typed-helper-request-red/` にある。

## 比べた案と決定

| 案 | 判断 |
|---|---|
| 診断文を残してホストで分類する | 採らない。文面への依存と失われる ID が残る |
| これらをプラグインの拒否にまとめる | 採らない。不正な要求とヘルパー内部の手順はプラグイン自身の拒否と異なる |
| 共有の `Refusal` に対象 ID と応答の種類を持たせる | 採る。分類と数値を通信路の両側で保持できる |
| 要求ごとに別のメインスレッド用チャネルを作る | 採らない。既存の所有・busy・時間切れの経路を分ける複雑さに見合わない |

性能改善は主張しない。
音声処理、状態の保存形式、時間切れ・busy・自動再送しない判断は変えない。

## 型と通信

- 予約済み ID の拒否を `Refusal::InvalidParameterId(u32)` で返す。
  ID の検査が終わるまで一括変更を公開しないため、先行する通常の値も適用されない。
  未知でも予約済みでない ID は従来どおりプラグインへ渡す。
- 非有限値は既存の制御ソケットのデコードで拒否する。
  `apply_params` の同じ検査は除き、型付きの ID 拒否だけをそこに置く。
  親の `SessionError::NonFiniteParameter` による送信前の拒否も維持する。
- 応答の不一致は `Refusal::UnexpectedMainReply { expected, actual }` で返す。
  共有の `MainReplyKind` は State・Done・Parameters・Values・Parsed・Window の 6 種類である。
  実際の `MainReply` からの変換を `main_requests.rs::unexpected` の網羅的な照合にまとめる。
  既存の 7 つの呼出しの不一致に同じ変換を使い、診断文は期待する応答から従来どおり選ぶ。
- 新しい拒否の種類は `9`（ID の LE u32）と `10`（期待・実際の各 1 バイト）である。
  応答の種類は 0〜5。欠けた数値、余分なデータ、未知の種類は `InvalidData` で拒否する。
  読めたバイトは同じバイトへ書き戻せる。同梱の同じ版だけが話す前提は ADR 0293 と同じである。

## 回帰と基盤

- `tests/isolation.rs::invalid_parameter_id_keeps_its_kind_and_rejects_the_entire_batch` は実ヘルパーを通し、
  拒否の型・診断文・変更前の状態の保持・予約済みでない境界 ID・後続の通常編集を照合する。
  本体にプラグインをロードせず、音声機器を開かない。
- `main_requests_tests.rs` は全ての期待・実際の異なる応答の組合せを照合する。
  `refusal_tests.rs` は ID の端点、応答の全組合せ、診断文、不正な長さ・種類の拒否と正規性を照合する。
  内部の応答不一致は通常の製品では生じないため、実ヘルパーへ故障注入の入口は増やさない。
- 制御ソケットの既存の fuzz 対象へ、新しい種類・ID の端点・全応答の組合せの種を足す。
  通常の `control_inputs_replay` でもデコードと往復を実行する。
- 領域別設計、コードの所在、既存の診断手順、監査の状態を更新する。
  既存の通常テストと検査の選択が追加回帰を実行するので、Makefile と `scripts/verification.json` の変更は不要である。
  新しい成果物・実機の確認場面は増えず、status.md とユーザー確認の一覧は変えない。

## 残る範囲

`Refusal::Other` は、`launch_refusal` が起動では届かない `HostError` を扱う防御分岐に残る。
次はこの到達範囲を整理し、M1 の残りを棚卸しする。今回を M1 全体の完了とはしない。
検査結果と計測・実機の見送り理由は `target/verification/report.md` を正本とする。
