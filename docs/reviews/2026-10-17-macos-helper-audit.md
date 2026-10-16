# 2026-10-17 macOS 境界の静的監査

指摘を、共有領域のハンドラ、Workgroup の受け渡し、objc2-foundation の稼働フラグ、ARCHITECTURE の AU と GUI、S3、ADR 0079 と照合した。指摘側は新しいテストやコマンドを実行していない。採用した変更の自動検査は `make verify` の結果を正とする。

## 採用

- `SIGABRT` だけの印では、`SIGSEGV`、`SIGBUS`、`SIGILL` のあとプロセス終了まで次のブロックがハングと区別できない。同じハンドラで印を書く。Mach 例外ポートは取らない。
- いまカーネル待ちのブロックは、印だけでは戻らない。締め切りか終了通知まで待つ。S3 の 123 ms から 1 秒超は書き換えていない。

## 採用しない

- `UserInteractive` に `LatencyCritical` が無い、という指摘。objc2-foundation では `UserInteractive` がすでに `UserInitiated | LatencyCritical` である。
- 非推奨の `bootstrap_check_in` / `bootstrap_look_up` を、測り直さずに別の受け渡しへ替えること。2 ms のポーリングは起動時の制御スレッドに限る。
- `sandbox_init` を `--scan` へ掛けること。公開の `libc` に無く、再生ヘルパーには使えない。

## 残す

AUv3 をヘルパーの中で読む二段の往復、プラグインウィンドウがキーウィンドウになる問題、スキャンプロセスの Seatbelt は、対応する製品経路がまだ無い。AU と GUI をこの監査では作らない。
