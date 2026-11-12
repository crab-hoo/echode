# 資料の入口

AI エージェントはまず [AGENTS.md](../AGENTS.md) を読む。
開始は `make agent-context`（個別確認は `make doctor` と `make verify-plan`）、終了は `make verify`、節目は `make verify-full`。
この索引は資料の役割を示し、検査の選択条件は持たない（[ADR 0094](decisions/0094-agent-development-foundation.md)、[ADR 0119](decisions/0119-agent-token-efficiency.md)）。
作業の開始時に全文を読むのは AGENTS.md・この索引・[現在の制約](status.md)・[エージェントの手順](agent-workflow.md) だけで、合計の上限を `make agent-context` が測る。
ほかの資料は見出しの一覧から必要な節だけを読む（[ADR 0235](decisions/0235-reading-budget-and-split-guides.md)）。

## 作業で使う資料

- [現在の範囲と制約](status.md): 利用者に残る制約と、未接続・未確認の項目。経緯は持たない。
- [ユーザー確認の一覧](user-checks.md): 実画面・VoiceOver・聴感・実機・第三者プラグインの確認待ちを場面ごとにまとめた正本（UC-番号）。
- [P1完了条件の最終照合](reviews/2026-10-17-phase1-acceptance.md): Phase 1 の 4 条件の製品経路・自動回帰・聴感確認、将来機能との境界。
- [Phase 2 の進め方](phase2.md): Phase 2 の実装単位（P2-01〜P2-09）の順番と各単位の受入条件。
- [制作ワークフローとインタラクションの検討](interaction-design.md): 制作フローの操作要件と未決事項の正本（[ADR 0117](decisions/0117-workflow-interactions.md)）。
- [開発手順](development.md): 作業の順番、[コードの所在](development.md#コードの所在)、回帰テストの追加方法、[基盤の保守](development.md#基盤の保守を同じ作業で行う)。
- [エージェントの情報取得と実行](agent-workflow.md): 一括の開始診断、根拠付きの節・行の読込、実行と引継ぎの原則。
- [検査手順](verification.md): 自動検査と実機検査、見送り、結果の有効期限と CI。必須契約の一覧は [scripts/verification.json](../scripts/verification.json) が正本。
- [デバッグ手順](debugging.md): 最初の診断、症状の索引、引継ぎに残す情報。症状ごとの手順は [領域別のファイル](debugging/) にある。
- [PLAN](PLAN.md): 製品の方針、ライセンス原則（[§2.1](PLAN.md#21-ライセンス方針の影響)）、ロードマップと完了条件。
- [ARCHITECTURE](ARCHITECTURE.md): 全体構成と設計原則の目次。領域別の詳細は [docs/architecture/](architecture/) 配下の分割ファイルを参照する。
  - [モデル・時間・Git 履歴](architecture/model-and-history.md)（§4〜§6）
  - [オーディオエンジン・DSP・解析](architecture/audio-engine.md)（§7、§8、§11）
  - [プラグイン隔離・内蔵デバイス](architecture/plugins-and-devices.md)（§9、§10）
  - [GPU・UI・Metal・ピアノロール](architecture/ui-and-metal.md)（§12〜§14）
  - [API / MCP・品質保証・ビルド配布](architecture/api-qa-and-distribution.md)（§15〜§17）
- [ADR](decisions/): 設計判断の記録。番号は時系列であり、置き換えた判断は後の ADR を参照する。
- [監査の索引](reviews/README.md): 監査と対応の ADR を領域ごとに並べた一覧。
- [スパイク](spikes/): 条件付きの測定結果（S1〜S5）。測定当時の機器・版・入力の範囲に限る。

## 監査の読み順

作業する領域に関係する監査と ADR だけを読む。全項目を毎回通読しない。
監査の一覧と、領域ごとの対応の ADR は [監査の索引](reviews/README.md) にある。
監査は発見時点の証拠を残すため、過去の「失敗」「未実施」を書き換えず、現在の合否は `target/verification/report.md` で確認する。
