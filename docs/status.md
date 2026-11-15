# 現在の範囲と制約

更新: 2026-11-15。
制約と未確認・未接続を置く。合格証ではない。
合否は `make verify`、実操作は日付付きの監査記録で確認する。
実機等の確認待ちは [ユーザー確認の一覧](user-checks.md) の UC-番号で示す。

対象は macOS 27 以降、M4 ファミリー以降の Apple Silicon である（[ADR 0112](decisions/0112-m4-minimum.md)、[ADR 0114](decisions/0114-m4-build-baseline.md)）。
起動時のチップ名による機種判定と SME 最適化は未実装で、実機性能の改善量は未確認である。

## 進捗

- **Phase 0（S1〜S5）**: 確定済み。
- **Phase 1**: 完了（[最終照合](reviews/2026-10-17-phase1-acceptance.md)）。
- **Phase 2**: 進行中。単位ごとの状態・受入条件・関連 ADR は [進め方](phase2.md) にある。
  - **P2-01（ヘッドレス MCP）**: 完了。GUI アプリへの接続・再生操作・外部クライアントの登録は未接続（[ADR 0115](decisions/0115-phase2-mcp.md)）。
  - **P2-02（デバイスビューとチェイン操作）**: 着手中。オートメーションは未接続で、P2-04 で扱う。
  - **P2-06（ミックス）**: 先行分だけを接続した。コンソール表示は未接続（[ADR 0228](decisions/0228-selected-track-live-mix.md)）。
  - **P2-07（マーカー・録音・ピアノロール）**: 着手中。
    複数トラックの同時編集・試聴・ドラム入力は未接続（[ADR 0181](decisions/0181-piano-reference-workflow-and-keys.md)、[ADR 0184](decisions/0184-piano-note-duplicate-drag.md)）。
- **監査記録**: 状態は各監査の表が正本。未完了の束ねは [統合表](reviews/2026-11-08-open-items-register.md)。
  後続の監査で指摘がなくても完了にはしない。

## 領域別の制約

制約・未接続・未確認の項目の正本は次のファイルに置く。触る領域の見出しと必要な節を読む。
`make agent-context` は変更領域に応じて案内し、開始時の全文読込には含めない。

- [保存・復旧・パッケージ](status/storage.md)
- [API・MCP・文書の入口](status/api-and-documents.md)
- [音声・再生・トランスポート](status/audio.md)
- [プラグイン・デバイスビュー](status/plugins.md)
- [ピアノロール](status/piano-roll.md)
- [アレンジ・ブラウザ・ミックス・画面](status/arrange-and-browser.md)

分割の判断と測定は [ADR 0370](decisions/0370-domain-status-guides.md) にある。
