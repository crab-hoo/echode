# ADR 0234: ジャーナルの不確定を文字列でなく型で判定する

- 日付: 2026-11-03
- 状態: 採用
- 関連: [横断監査](../reviews/2026-11-02-cross-cutting-audit.md)の XCA-16、[構造監査](../reviews/2026-10-30-structure-audit.md)の SPG-11、AUD-03

## 背景

開始時の HEAD は `2ecc601`。

`crates/echode-history/src/host.rs` の `note_failure` は、`error.to_string().contains(JOURNAL_DURABILITY_UNKNOWN)` で文書を止めていた。
ジャーナルの行がファイルに届き、その後の同期か巻戻しが終わらなかった時に、以後の編集を拒否する仕組みである（AUD-03）。

文面の照合なので、同じ語を含む別のエラーでも止まる。
たとえばパッケージのパスにその語があると、ただの書込みの失敗で文書が止まる。
逆に、文面を変えると黙って止まらなくなる。

## 決定

- `PackageError` に、不確定かどうかを持たせる。作るのは `PackageError::durability_unknown()` だけで、`is_durability_unknown()` で読む。
  文面は従来の `journal durability is unknown` のままで、利用者と API への表示は変えない。
- `append_journal` の不確定の 2 か所（故障注入の `Uncertain` と、同期の失敗後の巻戻しの失敗）はこの構築を使う。
- `note_failure` は `HostError::Package` の型の印だけを見る。ジャーナルの追記の誤りは包まずに `HostError::Package` で届く（`host.rs` の唯一の呼出し）。
- 戻しの失敗で止める経路（`invert_on_failure`）は従来どおり文面を記録するだけで、判定には使わない。

## 残る範囲

- `echode-actions` の `parameter_message` は serde のエラーの文面から自前の文を探す。
  照合する文は自分で作った `Error::custom` の文で、`crates/echode-api/src/rpc_tests.rs` が製品の経路で 3 つとも固定している。
  依存の更新で形が変われば、黙って変わらずにそのテストが落ちる。型にするには serde の外にエラーを運ぶ必要があり、今回は変えない。
- `Result<_, String>` の残り（約 155 か所、SPG-11）は、境界ごとに領域の作業に合わせて進める。

## 回帰

- `only_the_typed_durability_error_halts_later_edits`（`host_tests.rs`）: 同じ語を含む別のエラーでは止まらず、型の印のあるエラーでは止まる。
  修正前は、前半の `halted == None` で落ちた（印を足し、判定は文面のままの状態で確認した）。
- `audit_aud03_unknown_durability_stops_later_edits_until_recovery` は変えずに通る。
