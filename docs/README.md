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

監査は発見時点の証拠を残す。本文の過去の「失敗」「未実施」を消して現在の合格に書き換えない。対応状況と回帰テストへの参照を追記し、現在のソースの検査は `target/verification/report.md` で確かめる。`target/` の証拠はローカル生成物で、別の環境には存在しないことがある。
