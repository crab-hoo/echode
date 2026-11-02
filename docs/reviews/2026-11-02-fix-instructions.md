# 2026-11-02 監査対応の実行指示

この指示書は、次の 2 つの監査で見つかった欠陥を直す実行役への作業契約である。

- [横断監査](2026-11-02-cross-cutting-audit.md)（XCA-01〜22）
- [外部監査の精査](2026-11-02-external-audit-review.md)（DOC-A1 ほか）

指摘の根拠と再現の詳細は各監査にある。ここでは順番、触る場所、守る境界、完了条件だけを決める。

## 作業を始める前に

1. AGENTS.md を読み、`make agent-context` を実行する。担当する作業単位の監査の節と関連 ADR だけを読む。
2. 環境の状態を確かめる。
   - Claude Code は編集ツールが検査のロックを待ち、終了時に `make verify` の判定を受ける（`.claude/settings.json`）。
   - Codex のフックは開発者の絶対パスを指し、道具名も未確認である（XCA-06）。Codex では自分でロックを確かめる。
   - 作業ツリーは他のセッションと共有される。`make agent-context` の変更一覧には他の作業も入る。
     他の作業の失敗で `make verify` が通らない時は、その作業を直さず、待つかユーザーに相談する（XCA-07）。
   - 2026-11-02 時点の空き容量は約 9 GiB である。新しい worktree の初回の検査は 10 GB を超える。
     `make verify-full` の反例は一時 26 GB に達した。実行前に `df -h` を見る。キャッシュの削除はロックの解除後に、ユーザーと決める。
3. 修正の前に再現が失敗することを確かめる。
   - Rust の 6 件は `make known-bugs` で走る（下の表）。
   - Python の 2 件は [test_agent_hooks.py](../../scripts/tests/test_agent_hooks.py) の `RegisteredAuditReproductions` で、`skip` を外すと失敗する。
   - 保存の同期の順序は `python3 -B scripts/durability_trace.py --require-durable` で調べる。`cargo build -p echode-cli` の後に使う。今は終了コード 1 になる。
4. 意図した red の確認では、次の 2 点に注意する。
   - `checked_commands.py --expect-failure` の正規表現は、コマンド行に現れない文にする（ENV-06 が直るまで、`Command:` 行にも一致する）。
   - `--exact` にはモジュールのパスまで書く（例: `plugin_gui::audit_doc_a1_...`）。一致しないと 0 件の実行が成功になる。

| 再現 | 対象 | 場所 |
|---|---|---|
| `audit_doc_a1_a_dirty_change_in_an_open_window_counts_as_a_window_edit` | DOC-A1 | `crates/echode-plugin-host/tests/hosted_playback/plugin_gui.rs` |
| `audit_doc_a2_an_unrelated_edit_keeps_the_held_plugin_change` | DOC-A2 | 同上 |
| `audit_xca02_a_journal_gap_is_not_silently_dropped` | XCA-02 | `crates/echode-history/tests/audit_durability.rs` |
| `audit_xca03_reimport_repairs_a_damaged_object` | XCA-03 | 同上 |
| `audit_his_a1_a_missing_state_object_does_not_lose_later_edits` | HIS-A1 | 同上 |
| `audit_cli_a1_a_one_frame_plugin_export_completes` | CLI-A1 | `crates/echode-cli/tests/render.rs` |
| `test_audit_env01_deleting_unsafe_code_selects_the_sanitizers` | ENV-01 | `scripts/tests/test_agent_hooks.py` |
| `test_audit_env06_the_command_line_cannot_satisfy_an_expected_failure` | ENV-06 | 同上 |

## W1: プラグインの窓での変更（DOC-A1・DOC-A2）— P1、最初に行う

- 状態: 2026-11-02 完了。ヘルパーを作り直さない案を選んだ（[ADR 0229](../decisions/0229-plugin-window-live-edits.md)）。
- 場所: `crates/echode-sandbox/src/session.rs` の `Dirty` の腕、`crates/echode-document/src/saves.rs` と `worker.rs` の `turn`、
  `crates/echode-document/src/activity.rs` の `collapse`、`swift/EchodeApp/Sources/EchodeAppCore/PluginWindows.swift` と `DeviceParameters.swift` の `finishParameter`。
- 契約:
  - 窓が開いている間の `dirty` は窓での編集であり、静止の後に 1 回の Undo で記録する（ADR 0132）。
  - 無関係な編集・Undo・Redo は、記録されていないプラグインの変更を捨てない。
    先に記録するか、そのヘルパーを作り直さないかを選び、ADR に理由と一緒に残す。
  - 元の値へ戻したドラッグも、保存の保留を解く。
  - 活動の縮退でも、窓が開いていたことを失わない。
- 完了条件:
  - 上の 2 件の `#[ignore]` を外して通る。
  - 文書ワーカーか Swift で直した場合は、DOC-A2 の再現をその層の回帰へ移す。消すだけにしない。
  - `hosted_playback`、`PluginWindowTests`、`DeviceParameterTests`、関係する必須契約、`rtsan` が通る。
- 境界: 制御の通信形式、期限、DSP、保存形式を変えない。実画面と第三者プラグインでの確認はユーザーの担当として見送る。

## W2: 保存の耐久性（XCA-01・02・03、HIS-A1）— P1

- 状態: 2026-11-02 完了（[ADR 0230](../decisions/0230-save-durability.md)）。
- 場所: `crates/echode-history/src/` の `autosave.rs`（コミット）、`document.rs`（`commit_tree`・`compact_journal`・`contiguous_suffix`・`parse_track`）、
  `host.rs`（`publish`・`finish_saves`・`wait_for_saves`）、`objects.rs`（`write_object`）。プラグイン状態は `crates/echode-document/src/device.rs` からも書く。
- 契約:
  - ジャーナルを圧縮する前に、そのコミットで書いたオブジェクト・参照と、それぞれの親ディレクトリを同期する。gix は同期しない。
  - 保管物は、ディレクトリまで同期してから、それを参照する行を書く。
  - 中身が名前のハッシュと一致しない保管物は、同じ内容の取込みで置き換える。
  - 連番の欠け、`.git/echode-commit-chain` の最新より古い参照、状態の保管物の欠落は、ADR 0183 と同じ確認へ送る。
  - 自動保存は文書ワーカーを塞がない。同期を足した前後の自動保存の時間を測り、ADR に残す。
- 完了条件:
  - 上の 3 件の `#[ignore]` を外して通る。
  - `durability_trace.py --require-durable` が終了コード 0 になる。
  - ADR 0180・0183 の回帰と `crates/echode-history` の全テストが通る。
- 境界: 保存スキーマ 6、ジャーナルの形式、既存の復旧の同意の流れを変えない。実際の電源断は試さない。

## W3: 書き出しと走査のキャッシュ（CLI-A1、PLG-A1 (b)(c)）— P2

- 状態: 2026-11-02 完了（[ADR 0231](../decisions/0231-offline-export-and-scan-cache.md)）。
- CLI-A1: `crates/echode-cli/src/lib.rs` の `render_with_plugins`。オフラインの書き出しは期限なしで完了を待つ。
  ブロック長はテールを足した後に決める。再生のコールバックの期限は変えない。上の再現を通す。
- PLG-A1 (b): `crates/echode-plugin-host/src/scan.rs` の `store_scan_cache` を一時ファイルと名前変更にし、内容が同じなら書かない。
  `resolve_installed` の「走査しない」という説明を実際に合わせる。並行に解決する回帰を足す。
- PLG-A1 (c): 走査の標準出力に上限を置く。
- 完了条件: `crates/echode-cli/tests/render.rs`、`crates/echode-plugin-host` の走査の回帰、関係する必須契約が通る。

## W4: 検査基盤（ENV-01・06・02・05・07）— P2〜P3

- ENV-01: `scripts/verify.py` の `unsafe_changes` は、削除したファイルも差分の基準の内容から削除行を読む。
  `.c`・`.h` を対象にするかを決め、[検査手順](../verification.md) と ADR 0212 に書く。
- ENV-06: `scripts/checked_commands.py` は、ヘッダより後の出力だけを照合する。
- ENV-02・05・07: 横断監査と外部監査の精査の方針に従う。
- 完了条件: 上の 2 件の `skip` を外して通る。`scripts/verify.py` の変更は全自動検査を選ぶので、時間に余裕を見る。

## 実行役が決めないこと

次はユーザーの判断であり、指示があるまで行わない。

- リモートへのプッシュ（XCA-04）と Git の著者メール（XCA-22）。
- CI の有無（XCA-05）と、worktree を使うかどうか（XCA-07）。
- Codex のフックの修正（XCA-06）。設定を変えると Codex がフックを再び信頼する手順が要る。
- `target/` のキャッシュの削除、`make verify-full`、実機・聴感の検査。

## 共通の完了条件

- 1 つの作業単位を 1 つのコミットにする。他のセッションの変更を含めない。
- 直した項目の再現の `#[ignore]`・`skip` を外し、各監査の状態の列と `docs/status.md` の制約を更新する。
- 設計を選んだ箇所は ADR に残す。
- 最後に `make verify` を通す。実機・聴感を見送る時は、検査名と理由を残す。
- W1 と W2 が終わった時点で、`make verify-full` を行うかをユーザーに相談する。
- 残りの項目（XCA-08〜19、ENV-08、HIS-A2、ENG-A1、FFI-A1、SBX-A1 (2)）は、[横断監査](2026-11-02-cross-cutting-audit.md) の修正の進め方に従う。
