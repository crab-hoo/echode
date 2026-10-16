# 資料の入口

AI エージェントはまず [AGENTS.md](../AGENTS.md) を読む。開始は `make doctor` と `make verify-plan`、終了は `make verify`。この索引は資料の役割を示し、検査の選択条件は持たない。整理の判断は [ADR 0094](decisions/0094-agent-development-foundation.md)。

## 作業で使う資料

- [現在の範囲と制約](status.md): 実装済み、未接続、未検証を区別する。Phase 1 全体の完成を判定する入口。
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

監査は発見時点の証拠を残す。本文の過去の「失敗」「未実施」を消して現在の合格に書き換えない。対応状況と回帰テストへの参照を追記し、現在のソースの検査は `target/verification/report.md` で確かめる。`target/` の証拠はローカル生成物で、別の環境には存在しないことがある。
