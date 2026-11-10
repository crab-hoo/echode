# ADR 0310: 起動のエラーを実行中のエラーから型で分ける

日付: 2026-11-10
状態: 採用
関連: M1（DEV-7・SPG-11・XCA-16）、ADR 0295・0308・0309

## 問題と再現

開始時の HEAD は `d22a1e9`。
`launch_refusal(&HostError)` は読込み・生成・起動以外の 10 変種も受け取り、
起動では届かない場合を `Refusal::Other(error.to_string())` にしていた。
呼出し元を調べると、読込み、初回の activate、イベント容量の準備の 3 か所だけだった。
実行中のソケット・共有領域・Workgroup の失敗は、この返信の入口へ来ない。

未分類の拒否を削除する前に、製品の拒否デコーダーへ種類 `0` のフレームを通す回帰を足した。
空の文でも `Other("")` として受理され、「unclassified refusal frame was accepted」で失敗した。
期待終了コード 101 と固有のアサーションを照合した記録は `target/focused-checks/typed-launch-boundary-red/` にある。

## 比べた案と決定

| 案 | 判断 |
|---|---|
| 届かない分岐と `Other` を残す | 採らない。新しい呼出し元が原因を失ってもコンパイルできる |
| すべての `HostError` を通信へ追加する | 採らない。起動の手順と無関係な通信の種類を増やす |
| 届かない分岐で panic する | 採らない。型で防げる不一致を実行時の停止にする必要がない |
| 読込み・生成・起動の `LoadError` だけを返信の入口へ渡す | 採る。呼出し元と変換を型検査し、未分類の予備経路をなくせる |

性能改善は主張しない。
元の診断文・音声処理・busy・時間切れ・自動再送しない判断は変えない。

## 型と通信

- ヘルパーの `LoadError` は Entry・NoBinary・NoFactory・NoPlugin・NoPluginId・Instance の 6 変種である。
  `load_entry` と `load_plugin` はこの型を返す。
  activate は clack の原因を直接返し、初回だけ `LoadError::Instance` で包む。
- 実行全体の `HostError::Load` は元の `LoadError` を持ち、`Error::source` で clack の原因までたどれる。
  走査と計測も同じ読込みの型を使う。実行時の失敗を起動の型に変換する入口は作らない。
- `reject_launch` は `&LoadError` だけを受け取る。
  ポートとイベント容量の拒否は、元の `PluginFailure` を直接 `Refusal::Plugin` で送る。
- `Refusal::Other` と未分類の種類 `0` の読み書きを除く。
  同梱する同じ版だけが通信する前提は ADR 0293 と同じで、旧版用の互換経路は追加しない。
  残る種類の番号・ペイロードは変えず、種類 `0` は `InvalidData` で拒否する。

## 回帰と基盤

- `launch_tests.rs` は全インスタンス原因と構築できるエントリ原因について、
  通信の種類・診断文に加え、`HostError::Load` と `Error::source` の型の保持を照合する。
  `LoadError` の compile-fail の例は、実行中の `HostError` から変換できないことを固定する。
- 実ヘルパーを通す `tests/isolation.rs` の `launch_` を使う。
  ポート拡張がない拒否も `Plugin(NoAudioPorts)` のまま届く回帰を足す。
  既存の生成・起動・I/O・ネイティブローダーの拒否も同じ製品の入口を通す。
- 拒否の往復、要求番号、期限切れの古い返信、最後に適用した状態の保持の回帰は、型付きの fixture に移す。
  制御ソケットの fuzz の種にも未分類の種類 `0` を足し、通常の `control_inputs_replay` で再実行する。
- 領域別設計、コードの所在、既存の診断手順、監査の状態を更新する。
  既存の通常テストと選択条件が追加回帰を実行するため、Makefile と `scripts/verification.json` の変更は不要である。
  新しい成果物・実機の確認場面はなく、status.md とユーザー確認の一覧は変えない。

## M1 の棚卸しと次の境界

`check_typed_errors.py` の対象 11 crate では、製品コードの `Result<_, String>` と、
`map_err` 内の原因の文字列化の再導入を拒否している。
引数の文字列、表示用の報告、panic の payload、clack のネイティブ診断は、原因の種類とは別の値として残す。
Swift のアプリへ渡す表示の文や、開発用の Swift 編集型の生成器を、製品の未分類の拒否とは数えない。

棚卸しでは、検査の対象である `echode-sandbox/src/region.rs::shm_file` に、
`shm_open`・`shm_unlink`・`ftruncate` の errno を `io::Error::other(format!(…))` へ戻す 3 か所が見つかった。
`map_err` の外なので現行の静的検査は検出しない。
次はこの共有領域の境界で、元の errno と操作を保持し、静的検査の範囲も見直す。
今回を M1 全体の完了とはしない。
検査結果と見送り理由は `target/verification/report.md` を正本とする。
