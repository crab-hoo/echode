# ADR 0237: 最初のヘルパーの前に記述子の上限を上げる

- 日付: 2026-11-03
- 状態: 採用
- 関連: [ADR 0049](0049-hosted-playback.md)、[ADR 0236](0236-callback-work-per-track.md)

## 背景

開始時の HEAD は `3dea61a`。

[ADR 0236](0236-callback-work-per-track.md) の計測で、親はヘルパー 1 つにつき記述子を 4 つ持つと分かった。
1 トラックで 5、256 トラックで 1,025 である。
内訳は共有メモリのファイル、制御のソケット、標準エラーのパイプ、終了を見張る `kqueue` で、起動の途中ではソケットの対やパイプの複製が加わる。

Finder から起動したアプリは、launchd の既定の上限（`launchctl limit maxfiles` の soft 256）を受け継ぐ。
製品は `RLIMIT_NOFILE` を上げていなかった。AGENTS はプラグイン数に固定の上限を設けることを禁じている。

## 再現

`crates/echode-plugin-host/tests/descriptor_limit.rs` は、試験のプロセスの soft の上限を 256 にしてから、80 トラックのヘルパーを製品の再生の経路（`echode_engine::playback`）で起動する。
変更前は、`dup pipe: Too many open files (os error 24)` で起動に失敗した。
試験はプロセス全体の上限を変えるため、ほかの試験と混ぜない専用のバイナリにした。

## 決定

- `PluginHost::spawn` の最初に、プロセスで 1 回だけ soft の上限を上げる（`echode-sandbox/src/descriptors.rs`）。
  ヘルパーを起動する経路はアプリ・CLI・MCP のどれもここを通るので、入口ごとに足さない。
- 上げる先は `min(hard, kern.maxfilesperproc)` とする。断られたら、setrlimit(2) が受け付けるとする `min(OPEN_MAX, hard)`（10,240）にする。
  hard の上限は変えない。今の soft がそれ以上なら下げない。
- 上げられなくてもエラーにしない。ヘルパーの起動が後で失敗した時に、その原因の文が出る。

この機械の `kern.maxfilesperproc` は 92,160 である。記述子の分だけなら 2 万を超えるヘルパーを持てる。

## 検査

- 必須契約 `helper-descriptor-limit` が上の試験を実行する。反例は上限を上げる呼出しを消し、変更前と同じ起動の失敗になる。
- 再生した 1 ブロックに欠けたヘルパーがなく（`plugin_missed_blocks` が 0）、soft の上限が 256 より上がっていることも確かめる。

## 残る範囲

- 親はヘルパー 1 つにつきスレッドを 2 つ持つ（終了の見張りと標準エラーの読取り）。プロセスあたりのスレッドの上限（`kern.num_taskthreads`、この機械で 6,144）で、約 3,000 のヘルパーが次の限界になる。
- 管理者などが hard の上限を低くした環境では、その値までしか上げられない。
- Finder から起動した実際のアプリでの確認はしていない。試験は同じ soft の上限を試験のプロセスに作ったものである。
