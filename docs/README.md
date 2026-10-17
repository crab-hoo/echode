# 資料の入口

AI エージェントはまず [AGENTS.md](../AGENTS.md) を読む。開始は `make doctor` と `make verify-plan`、終了は `make verify`。この索引は資料の役割を示し、検査の選択条件は持たない。整理の判断は [ADR 0094](decisions/0094-agent-development-foundation.md)。

## 作業で使う資料

- [P1完了条件の最終照合](reviews/2026-10-17-phase1-acceptance.md): 4条件の製品経路・自動回帰とユーザーの聴感報告、将来機能との境界。
- [現在の範囲と制約](status.md): 実装済み、未接続、未検証を区別し、Phase 1 の完了と Phase 2 の進捗を確認する入口。
- [Phase 2 の進め方](phase2.md): 移行後の実装順、最初の MCP と後続の制作機能の受入条件。全体の範囲は PLAN を正とする。
- [開発手順](development.md): 作業の順番、コードの所在、回帰テストの追加方法。[基盤の保守](development.md#基盤の保守を同じ作業で行う) も各作業の完了条件に含める。
- [検査手順](verification.md): 自動検査と実機検査、見送り、結果の有効期限、フックと CI。
- [デバッグ手順](debugging.md): 環境・ソース・アプリ・検査を切り分け、再現と診断資料を残す。
- [PLAN](PLAN.md): 製品の方針、ロードマップと完了条件。
- [ARCHITECTURE](ARCHITECTURE.md): 設計と境界。将来の設計も含むため、存在するコードと ADR を照合する。
- [ADR](decisions/): 判断の理由。番号は時系列であり、置き換えた判断は後の ADR を参照する。
- [スパイク](spikes/): 条件付きの測定結果。測定当時の機器・版・入力の範囲に限る。

## 監査の読み順

1. [2026-10-08](reviews/2026-10-08-audit.md): HIS / MOD / ENG / PLG / API / UI / DEV の一覧と未着手項目。古い行番号・テスト件数は当時の記録。
2. [2026-10-13 の境界監査](reviews/2026-10-13-audit.md): AUD-01〜13。保存・隔離・書き出しの問題と対応。
3. [Phase 1 の専門監査](reviews/2026-10-13-phase1-pro-audit.md): PRO-01〜17 と未完成範囲。
4. [修正指示](reviews/2026-10-14-phase1-fix-instructions.md)、[修正後監査](reviews/2026-10-14-phase1-fix-reaudit.md): 修正の条件と再現結果。
5. [Computer Use](reviews/2026-10-14-computer-use.md)、[取得エラーの再調査](reviews/2026-10-14-computer-use-reinvestigation.md)、[同じデスクトップでの再監査](reviews/2026-10-14-computer-use-same-desktop.md): CU 系の製品と検査環境の問題。末尾の後日対応も読む。
6. [ADR 0093](decisions/0093-product-contract-gate.md): CU-03〜06 の修正・必須回帰検査と保証の限界。
7. [ADR 0095](decisions/0095-product-audio-buses-and-continuity.md): ステレオ、ループ継ぎ目、フェーダーの製品接続と、振幅を確認する必須契約。
8. [ADR 0096](decisions/0096-measurement-completion.md): 機器切替やコールバック停止・中断を、計測の合格にしない。

9. [ADR 0097](decisions/0097-cancellable-retained-transport.md): 準備中の停止と出力の再利用。
10. [ADR 0098](decisions/0098-pane-render-boundaries.md): ペイン変更中の描画境界。

11. [ADR 0099](decisions/0099-transport-plugin-state.md): 停止・位置移動時の発音状態の消去と、自然な余韻との区別。
12. [ADR 0100](decisions/0100-scoped-verification.md): 検査ごとの結果再利用と、通常の短時間確認・明示する長時間計測の分離。

13. [ADR 0101](decisions/0101-live-output-discontinuities.md): 定期保存確認による一時バイパスの排除と、ライブ出力の停止時の段差。

14. [ADR 0102](decisions/0102-exclusive-audio-graph.md): 音声グラフの排他的な所有と、再生を続けながら行う容量・構造変更。

15. [ADR 0103](decisions/0103-live-stop-waveform.md): 停止時に鳴っている波形を減衰する処理と、実機・聴感検査の担当。

16. [ADR 0104](decisions/0104-stop-output-diagnostics.md): 停止クリックの再報告、停止中のループフェード維持、実出力の任意の記録。

17. [ADR 0105](decisions/0105-nonblocking-stop-cleanup.md): 実出力で判明した減衰後の不要な待機と、待たない後片付け・再開の保留。

18. [ADR 0106](decisions/0106-natural-plugin-onsets.md): Vital Initの自然な発音と直接出力の比較、余分な変化を検出する必須回帰。

19. [ADR 0107](decisions/0107-mix-controls.md): トラックと音声クリップの画面操作、試聴と1回のUndo、フェーダー後のピーク。

20. [ADR 0108](decisions/0108-phase1-loop-readiness.md): ループ先頭の事前準備と保持、製品のフェーダー前の差し替え口。
21. [ADR 0109](decisions/0109-placement-bounds-and-plugin-capture.md)、[2026-10-17 の境界再監査](reviews/2026-10-17-boundary-reaudit.md): 配置の原点、プラグイン状態の取り込み、クリップゲインの上限、停止中のキャッシュ範囲。
22. [ADR 0110](decisions/0110-apple-silicon-draw-and-peaks.md)、[2026-10-17 の Apple Silicon 監査](reviews/2026-10-17-apple-silicon-audit.md): 画面のリフレッシュレート、波形バッファの再利用、ピークキャッシュの部分読出、稼働表明。
23. [ADR 0111](decisions/0111-macos-helper-notices.md)、[2026-10-17 の macOS 境界監査](reviews/2026-10-17-macos-helper-audit.md): 致命信号の印、Workgroup の起動時ポーリング、未実装の AU・GUI・スキャン制限。
24. [2026-10-17 の macOS 27 最適化監査](reviews/2026-10-17-macos27-optimization-audit.md): 製品ヘルパーへの Workgroup の未接続、Rust の最低 OS、配置 PCM の重複、製品と S2 の並列化の境界、描画と Metal 4 の比較候補。
25. [ADR 0112](decisions/0112-m4-minimum.md): ユーザー指定による M4 ファミリー以降への対象変更。旧監査の M1 基準を置き換え、M4 向けビルドと SME / SME2 を比較候補にする。性能の実装・計測とは区別する。

26. [ADR 0113](decisions/0113-product-macos-workgroup-and-caches.md): 製品の Workgroup 接続と再起動・変更通知、M4 / macOS 27 のビルド・配布契約、PCM と可視列の再利用。

監査は発見時点の証拠を残す。本文の過去の「失敗」「未実施」を消して現在の合格に書き換えない。対応状況と回帰テストへの参照を追記し、現在のソースの検査は `target/verification/report.md` で確かめる。`target/` の証拠はローカル生成物で、別の環境には存在しないことがある。

- [ADR 0114](decisions/0114-m4-build-baseline.md): 全ての自前のビルド対象の M4 基準、Swift / Clang の設定の一致、性能比較による例外。
- [ADR 0115](decisions/0115-phase2-mcp.md): Phase 2 の開始と、既存のヘッドレスセッションを使う最小 MCP、プロトコルの境界と回帰。
