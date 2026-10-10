# Echode を変更するエージェントへ

Echode は macOS 専用の DAW です。方針は `docs/PLAN.md`、設計は `docs/ARCHITECTURE.md`、決めたことは `docs/decisions/` にあります。実装に入る前に、触る領域の節を読んでください。未解決の指摘は `docs/reviews/2026-10-08-audit.md` に ID 付きでまとめてあります。作業の順番と、触る領域の項目も読んでください。

## 言語

- コード、識別子、コードコメント、コミットメッセージは英語
- 計画書、設計書、ADR、このファイルは日本語

## 今あるもの

Phase 0（S1 から S5）は確定済みです。今は Phase 1 のアプリの骨組みまで進んでいます。設計書にある残りの crate は、その機能に着手する時に追加します。空の crate を先に並べないでください。

- `echode-sandbox`: プラグインプロセスとの共有メモリ
- `echode-plugin-host`: ヘルパープロセス。プラグインフォルダの検知もここ。本体はバンドルを読み込まない
- `echode-test-plugin`: クラッシュとハングを起こせるテスト用 CLAP
- `echode-draw`: 描画検証用のシーンと可視判定。Swift 側は `spikes/draw`
- `echode-history`: プレーンテキストのプロジェクトと gix による自動コミット。製品パッケージの保存とジャーナル復旧もここ。1,000 トラックの計測用の型は `spike`。計測は `make spike-history`
- `echode-engine`: オーディオグラフ、再生位置、MIDI とメトロノーム、テスト用シンセ（wavesynth の Sine Pad）とサンプラー（dasp の補間）、サンプルレート変換、キャッシュへ差し替えられるトラック、ループの継ぎ目、フェーダー、出力デバイスの追従。10 分間の計測の型は `spike`。計測は `echode-engine-spike`
- `echode-audio-io`: 製品の CoreAudio 出力、機器の一覧と通知、システムの出力への追従。制御スレッドで機器を切り替え、コールバックは準備した再生計画だけを読む。無音の実機検査は `make spike-audio`（画面は開かない）
- 入力の検証は Rust の crate ではなく `spikes/input`。`make spike-input`
- `echode-model`: ドキュメントモデル。時間、種別のないトラック、スマートクリップ、フォルダ（親 ID。バス処理は Phase 2）
- `echode-actions`: モデルを変えるアクション、Undo / Redo、再生できるジャーナル
- `echode-cli`: `echode render` と `echode serve`。書き出しは float WAV、操作は JSON-RPC
- `echode-api`: ローカルセッションの JSON-RPC。ソケットは本人だけが読み書きできる
- `echode-ffi` と `swift/EchodeBridge`: 製品の C ABI、非同期の編集・問い合わせと不変のスナップショット
- `swift/EchodeApp`: 3 ペインの開発用アプリ。Metal のアレンジとピアノロール（鍵盤・階名、直接編集と数値入力、画面外までのノート移動）、Undo / Redo、再生・停止・位置移動と音声出力、ファイル一覧とコマンドパレット。`make app`、実機の描画検査は `make spike-app`

## 守ること

- オーディオスレッドでは、ロック、メモリ確保、ファイル I/O、システムコールによる待機、GPU の完了待ちをしない。締め切りを持つ、プラグインのヘルパーとの往復の待ちだけは許す
- メモリ確保は、デバッグビルドのテストと、検出を入れたバイナリで止める。ロックと、ブロックするシステムコールは `make rtsan` で検出する
- トラック数、プラグイン数、チャンネル数、ネストの深さに固定の上限を設けない。検証コードのバッファ長は、その検証の範囲に限定していることをコメントに書く
- プラグインは別プロセスで動かす。本体のプロセスにプラグインを読み込まない
- 依存ライブラリは GPL-3.0-or-later と両立するライセンスに限る（寛容なライセンス、MPL-2.0、LGPL、GPL-2.0-or-later、GPL-3.0、AGPL-3.0）。GPL-2.0-only とソース非公開のライブラリは使わない。詳細は `docs/PLAN.md` §2.1
- 定番の実装がある処理（サンプルレート変換、タイムストレッチなど）は、自作する前に GPL と両立する既存の実装を調べて測る。ほかの選択肢に完全に劣るものを除き、できるだけ多くをユーザーが選べる選択肢にする。自作は、既存のものが要件を満たさない時か、測って上回る時に限り、理由と測定値を ADR に書く。詳細は `docs/PLAN.md` §2.1
- 非公開の Apple API を使わない
- ユーザーに頼まれていないコミットやプッシュはしない

## 書く時に避けること

2026-10-08 の監査で見つかった誤りの型です（詳細は監査の文書の §5）。多くは `make check` が止めます。

- 同一性をアドレスの値で比べない。`Arc` を持ったまま `Arc::ptr_eq` で比べる（clippy が `Arc::as_ptr` を止める）
- 音をブロックの切り方に依存させない。プロジェクト上のサンプル位置から決める。音を作る処理を足したら、`crates/echode-engine/tests/block_size.rs` にブロック長を変えて比べるテストを足す
- オーディオスレッドで、プロジェクト全体の大きさに比例する処理をしない
- OS の API を新しく手で宣言しない。`libc` か objc2 の crate を使う（`scripts/check-ffi.sh` が止める）
- モデルを変える操作は `Project::check_invariants` を守る。アクションを足したら、`crates/echode-history/tests/properties.rs` の操作の生成に足す
- 時間を測るテストと、実機が要るテストは `#[ignore]` にし、`make spike-*` で動かす

## 確認

着手時に `make verify-plan`、変更を終えたら `make verify` を実行します。変更箇所と追加検査の対応は `scripts/verification.json` だけに置きます。通常の `make check`（フォーマット、clippy、Rust / Swift、FFI、ライセンス）に加え、橋渡しの全 feature・ASan・TSan・Miri、音声の RTSan、入力の単体テストを必要な範囲で選びます。依存するモデルの変更も対象です。実行方法と検査範囲は `docs/verification.md` を読んでください。

計測や実機、ネットワークが必要な検査は、`scripts/verify.py --run ID` で実行するか、`--defer ID --reason 理由` で未実行の理由を記録します。自動検査の失敗を見送ってはいけません。未実行を合格と言わず、最終報告に検査名と理由を残します。結果は `target/verification/report.md` にあり、ソースやツールが変われば古い合格と見送りは無効です。

Cursor の stop hook（`.cursor/hooks.json`）も同じ入口を使い、失敗や理由のない未実行があれば続きの指示を返します。フックのない環境でも `make verify` の実行は必要です。コミット済みの変更は `VERIFY_BASE=<変更前のref> make verify`、全体の確認は `scripts/verify.py --all` を使います。`--only` は個別検査や CI の分割用で、作業の完了時には使いません。新しい環境では、先に `make setup`、橋渡しの動的検査には `make setup-bridge` を実行します。

監査の未解決の不具合は、`audit_<ID>_...` という名前の無視したテストになっています。`make known-bugs` で、まだ失敗していることを確かめられます。項目を直したら、そのテストの `#[ignore]` を外し、監査の文書の一覧の状態を更新します。

プラグインの隔離に触ったら、選ばれる `spike-isolation` を実行し、`docs/spikes/S3-plugin-isolation.md` の数値を更新します。新しい検査を追加したら Makefile と `scripts/verification.json` に登録し、選択のテストも更新します。基準音声の再生成（`make bless`）と、失敗が予定されている `make known-bugs` は自動の検査に混ぜません。
