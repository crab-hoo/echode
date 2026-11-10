# ADR 0308: ヘルパーの起動拒否を種類と診断文に分けて送る

日付: 2026-11-10
状態: 採用
関連: M1（DEV-7・SPG-11・XCA-16）、ADR 0293・0295・0307

## 問題と再現

開始時の HEAD は `8278bc5`。
ヘルパー内部の読込み・作成・起動のエラーは型だが、`threads.rs::reject_launch` が `Refusal::Other(error.to_string())` に戻していた。
本体は、指定した ID がない場合・生成の拒否・起動の拒否・ファイルの I/O を、診断文でしか区別できなかった。

製品の `PluginHost::spawn` と同梱の実ヘルパーを通す回帰を先に足した。
存在しない ID を指定すると `Refused(Other("… does not contain a plugin"))` となり、種類のアサーションで失敗した。
期待終了コード 101 と回帰固有の文を照合した記録は `target/focused-checks/typed-helper-launch-red/` にある。

## 比べた案

| 案 | 判断 |
|---|---|
| ADR 0295 のまま診断文を送る | 採らない。文から分類する境界が残る |
| 本体と共有する crate に clack のローダーを依存として足す | 採らない。ロードするのはヘルパーだけであり、親へ不要な依存を広げる |
| 共有する原因の種類と必要な数値を送り、元の診断文を表示用として添える | 採る。分岐は型で行い、プラットフォーム固有の診断を失わない |
| 共有型の `Display` で clack の診断文を再実装する | 採らない。同じ文言を二つの層で保守することになる |

性能改善は主張しない。
音声スレッド、保存形式、再試行・復旧の判断、走査の期限は変更しない。

## 決定と通信の境界

- `echode-sandbox` に `LaunchFailure { kind, diagnostic }` と `LaunchFailureKind` を置く。
  `kind` は 27 種類で、エントリの原因・バイナリ／factory／ID の欠落・インスタンスの原因を分ける。
  I/O の errno、NUL の位置、互換でない CLAP のプラグインとホストの版は型付きの値として持つ。
  パスを含む元の診断は `diagnostic` に保持し、分類に使わない。
- clack の型からの変換は `echode-plugin-host/src/launch.rs` だけに置く。
  インスタンスの列挙型は全変種を照合する。
  エントリの列挙型は upstream が非網羅なので、将来の未知の変種だけを `UnknownEntry` として保持する。
  ローダーのネイティブな原因はヘルパー内部の `HostError` に引き続き残る。
- 起動の失敗は `Refusal::Launch` で送り、親は `SessionError::Refused` で受ける。
  stderr がある時の外側の `SessionError::Launch` と、表示文は変えない。
  ポートと容量の拒否は従来どおり `Refusal::Plugin` にする。
- 拒否の種類 `8` の後に原因の番号（0〜26）、その種類に必要な固定長の数値、UTF-8 の診断文を置く。
  数値は LE で、errno は有無の 1 バイトと、ある場合だけ i32 である。
  不正な番号・errno のフラグ・欠けた数値・非 UTF-8 を拒否し、読めたものは同じバイトへ書き戻せる。
  アプリと同じ版のヘルパーだけが話すため、旧版への互換層は増やさない（ADR 0293）。

## 回帰と基盤

- `tests/isolation.rs` の `launch_` は実ヘルパーを通し、ID 不一致、factory の生成拒否、起動拒否、errno、ライブラリのネイティブ診断を照合する。
  `generate` の fixture は factory の `create_plugin` が null を返すため、clack の原因は `InstantiationFailed` でなく `PluginNotFound` である。
  通常の manifest ファイルをライブラリとして読む検査もヘルパーの中だけで行い、本体へプラグインをロードしない。
- `launch_tests.rs` は clack の全インスタンス原因と、構築できるエントリ原因・数値・診断文の保存を照合する。
- `launch_failure_tests.rs` は全原因と数値の端点の往復、欠けた数値・不正な番号・UTF-8 の拒否、受理したバイトの正規性を照合する。
  既存の `refusal_tests.rs` と隔離回帰の busy・時間切れ・プラグインの拒否も維持する。
- 制御ソケットの既存の fuzz 対象がこの読込みも通す。
  新しい種類と全数値の形の種を足し、通常の `control_inputs_replay` でも実行する。
- 設計書、コードの所在、既存の CLAP のデバッグ手順、監査の状態を更新する。
  既存の通常テストが追加の回帰を実行し、選択条件も有効なので、Makefile と `scripts/verification.json` に新しい検査を増やさない。
  新しい成果物・診断の収集先・機器操作はなく、status.md とユーザー確認の項目は変えない。

## 残る範囲

`Refusal::Other` に残るのは、無効なパラメーター ID と、メインスレッドの応答などヘルパー自身の手順の食い違いである。
`launch_refusal` の起動では届かない `HostError` の変種も、元のヘルパー診断を残す扱いである。
次はその境界を扱い、M1 全体の完了とはしない。
検査結果と計測・実機の見送り理由は `target/verification/report.md` を正本とする。
