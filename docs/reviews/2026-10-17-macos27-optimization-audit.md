# 2026-10-17 macOS 27 以降への最適化監査

本文は監査時の発見と証拠を保持する。M4 以降への前提変更に続き、M27-01 / 02 / 03 と M27-04 の一部へ対応した。現在の実装は末尾の「後日対応」と [ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)、合否は検査レポートを参照する。

- 対象: `276792b1a0d0d84e296fcaa638a4406f56426da1` の製品実装、PLAN、ARCHITECTURE、関連 ADR。監査開始時の作業ツリーは変更なし
- 開始時の前提: macOS 27.0 以降、Apple Silicon 専用。旧 OS、Intel、Rosetta への対応は追加しない。その後のユーザー指定により、現方針は M4 ファミリー以降（末尾の追記、[ADR 0112](../decisions/0112-m4-minimum.md)）
- 環境: macOS 27.0（26A428）、arm64、Xcode 27.1（27A9275）、macOS SDK 27.0、Rust 1.96.1
- 範囲: 静的な製品経路の照合、生成したバイナリの確認、機器を開かない PCM 再現と隔離回帰。製品コードは変更しない

## 結論と順番

Mac 向けの土台はすでにある。Swift の最低 OS と Info.plist は 27.0、描画は CAMetalDisplayLink と共有 Metal バッファ、IPC は共有メモリと `os_sync_wait_on_address`、素材デコードは AudioToolbox である。古い OS 用の availability 分岐が大量に残っている状態ではない。

一方で、**Audio Workgroup の仕組みが製品再生につながっていない**。ここが最優先である。次に Rust の最低 OS を揃え、素材の重複準備と、再生中も繰り返す静的な描画準備を減らす。製品 CPU グラフの並列化、ヘルパーの集約、Metal 4 は、効果と複雑さを比較して採用する候補である。

以下の P1 / P2 は対応の優先度であり、速度の改善を測定済みという意味ではない。今回の再現は PCM の所有・内容と機能の回帰であり、音切れ、CPU 使用率、消費電力、描画時間の改善量は測っていない。

## M27-01 / P1: 製品の CLAP ヘルパーへ Workgroup を渡していない

**確認済みの接続漏れ。未修正。**

- `echode-audio-io/src/stream.rs` の `open_with_epoch` → `prepare_playback` → `open_playback` は、選択したデバイスの ID / レート / バッファ長から製品再生を作るが、デバイスの Workgroup を取得・保持・受け渡ししない。
- [plugins.rs](../../crates/echode-engine/src/plugins.rs) の `spawn_stage` は、`SpawnOptions { workgroup: None, ... }` で製品の各ヘルパーを起動する。
- [threads.rs](../../crates/echode-plugin-host/src/threads.rs) の `audio_thread` は `Some` のときだけ `join_passed_workgroup` を呼ぶ。時間制約と QoS の設定も [workgroup.rs](../../crates/echode-sandbox/src/workgroup.rs) の `prepare_thread` にあり、その参加経路に属する。したがって今回追った製品の起動経路では、この設定も行わない。
- `IoWorkgroup`、送信権の受渡し、S2 / S3 の検証が存在することでは、この製品接続は保証できない。I/O スレッド自体は CoreAudio が管理する。欠けているのは補助ヘルパーの接続である。

Apple は、I/O と同じ締め切りで処理する補助スレッドをデバイスの Workgroup に加えるよう説明している。別プロセスも対象になる。根拠: [Adding Parallel Real-Time Threads](https://developer.apple.com/documentation/audiotoolbox/adding-parallel-real-time-threads-to-audio-workgroups)、[Meet Audio Workgroups](https://developer.apple.com/videos/play/wwdc2020/10224/)。

**対応案と完了条件**: 選択中の HAL デバイスから制御側で Workgroup と周期を取得し、保持した所有者から製品ヘルパーへ渡す。既定デバイスを別に開く `IoWorkgroup::open` をそのまま流用しない。機器・周期の変更、保持した出力の再利用、再生成、参加失敗とポート解放を含める。オフライン書き出しは別のモードとして区別する。機器を開かない製品準備の検査で「ライブでは必ず渡る」「参加失敗を無言で成功にしない」をまず固定し、最後にユーザー担当の実機検査で負荷時の期限超過を比較する。今回、音切れの原因だと断定したものではない。

## M27-02 / P2: Rust は macOS 11.0 の既定ターゲットのまま

**ビルド設定と生成物で確認。未修正。**

- Swift の両 Package.swift とアプリの `LSMinimumSystemVersion` は 27.0。
- Cargo の設定に最低 OS の指定がなく、`build-app.sh` / `build-dist.sh` も `MACOSX_DEPLOYMENT_TARGET` を Rust のビルドへ設定しない。Xcode のアプリ設定だけでは、通常の Cargo 実行すべてへは伝わらない。
- 環境変数が未設定のこの環境で `rustc --print deployment-target` は `MACOSX_DEPLOYMENT_TARGET=11.0`。
- 現行ソースで `cargo build --locked --offline -p echode-plugin-host --bin echode-plugin-host` を実行後、`xcrun vtool -show-build target/debug/echode-plugin-host` は `minos 11.0 / sdk 27.0`。SDK の版と最低 OS の版は別である。配布版バンドルは今回作っていないため、その生成物を検査済みとはしない。

[Rust の公式説明](https://doc.rust-lang.org/rustc/platform-support/apple-darwin.html)も、最低 OS を上げると追加の最適化を得られる可能性を記している。ただし 27.0 へ揃えるだけの性能差は未測定である。

**対応案と完了条件**: Cargo・C 依存・Swift・ヘルパー・CLI・検査・配布の最低 OS と arm64 を一つの方針に揃える。外部環境による古い値の上書きも検出する。配布検査で plist だけでなく Mach-O の architecture と `LC_BUILD_VERSION` を確認する。LTO、codegen-units、PGO は別々にリリース性能を比較する。`target-cpu=native` を配布全体へ指定して開発機の命令へ固定することは避ける。

## M27-03 / P2: 同じ音声素材を配置ごとに読み直し、別の PCM 平面へ複製する

**製品ローダーで決定的に再現。未修正。既知の全素材常駐制約を具体化。**

[clips.rs](../../crates/echode-audio-io/src/clips.rs) の `load_placed_clips` は配置ごとに、同じハッシュでも次の処理をする。

1. `read_pcm` でオブジェクト全体を読み、検証して interleaved PCM を作る。
2. チャンネルごとの `source: Vec` を作る。
3. 同じレートでも別の `device` 平面へコピーする。
4. 配置の長さの平面をさらに作り、ゲインを焼き込んで `Arc` にする。

`media_loader` は前回と同じ配置集合なら再利用するが、配置・ゲインなどが変わって鍵が失効すると、この全体準備を繰り返す。配置間の同一素材の共有キャッシュではない。

**再現**: 48 kHz、4096 フレーム、左右が +0.5 / -0.5 のステレオ WAV を `import_audio` で取り込み、同じ `AudioRef` を同じトラックの 0 拍と 1 拍へ配置する。不変条件を確認後、`load_placed_clips(..., 48000)` を呼んだ。出力は左右とも配置間で全サンプル一致するが、両 `Arc` を保持した `Arc::ptr_eq` は左右とも false。各配置に 4096 × 2 × 4 = 32768 bytes、2 配置のサンプル本体は **65536 bytes**。これはサンプル平面の長さからの算出であり、プロセス RSS や一時確保のピークではない。

再現用ソースとログはローカルの `target/macos27-audit/src/main.rs`、`probe.log`。実行は `CARGO_TARGET_DIR="$PWD/target" cargo run --offline --manifest-path target/macos27-audit/Cargo.toml`。`target/` は他の環境には存在しない。一時プロジェクトだけを使い、音声デバイスは開かない。

**対応案と完了条件**: ハッシュ・デバイスレート・変換方式に基づく不変 PCM の共有を先に比較する。配置は範囲とゲインを持ち、加工結果が同じ配置は共有できる形にする。同レートのコピー省略、編集時の部分失効、ストリーミングの順に検討する。mmap をコールバックへ直接持ち込むだけでは、ページフォールトやファイル切詰めを解決しない。ハッシュ検証、ループ先頭の保持、負オフセット、トリム、フェード、ブロック長に依存しない音、準備中の停止を維持する。

## M27-04 / P2: 静止した画面の内容も、再生フレームごとに組み直す

**実装を確認。性能差は未測定。ADR 0110 で残した処理の継続。**

[CanvasRenderer.swift](../../swift/EchodeApp/Sources/EchodeAppCore/CanvasRenderer.swift) は表示リンクごとに `ArrangeFrame` / `PianoFrame` を生成する。アレンジは可視要素の問い合わせ、Swift 配列への変換、波形列の再生成、複数回の `filter` / `map`、インスタンスの連結を経て共有 Metal バッファへコピーする。[ArrangePainter.swift](../../swift/EchodeApp/Sources/EchodeAppCore/ArrangePainter.swift) も波形列を一度 Swift 配列へまとめ直す。停止中は表示リンクを休止する仕組みがあり、常時無駄に描いているという指摘ではない。

**対応案と完了条件**: スナップショットの版・カメラ・サイズ・倍率・選択などで静的な可視列・波形・文字・背景を保持し、再生位置とピークだけをフレームごとに更新する。描画・ヒットテスト・VoiceOver が同じ版を使う契約と、飛行中の 3 枠の所有を守る。共有メモリがあるだけで、CPU 側のコピーが自動で消えるわけではない。古い表示や寿命の危険を増やす `bytesNoCopy` の単純置換はしない。

## M27-05 / P2: S2 の並列 CPU グラフは製品の再生器ではない

**製品とスパイクの境界を確認。追加最適化の候補であり、P1 の完了を取り消す指摘ではない。**

[graph.rs](../../crates/echode-engine/src/graph.rs) の依存カウンタ・Workgroup ワーカーは、100 チェイン × 10 フィルターという S2 の負荷である。[spike.rs](../../crates/echode-engine/src/spike.rs) も Phase 0 の計測と明記する。製品の [playback.rs](../../crates/echode-engine/src/playback.rs) の `Prepared::render` は、内蔵シンセ、配置 PCM の処理、フェーダーと加算を I/O 側でトラック順に処理する。

一方、**製品の独立した CLAP トラックはすでに同時に投入する**。`HostGraph::collect` は完了した段から次段へ進み、共有の通知と一つの締め切りを使う。すべてのプラグインを直列で待っているわけではない。今回 `audit_aud07_one_deadline_collects_a_healthy_track_beside_hangs` を実行し、正常なトラックがハングしたトラックと区別される既存の回帰を確認した。

**対応案と完了条件**: 内蔵 DSP・配置処理が製品の CPU 時間を占めた段階で、製品の実行計画へ依存グラフを接続し、単一スレッドと比較する。短いブロックでは起床と同期が逆効果になり得る。S2 の速度を製品の達成値にせず、Workgroup の推奨並列数にはヘルパーも含めた実負荷を考慮する。加算順序、音声の一致、確保禁止、RTSan を維持する。

## M27-06 / P2: 同じチェインでも 1 デバイスごとに別ヘルパー

**ADR 0049 の既知の制限。今回の新規発見とは数えない。**

[plugins.rs](../../crates/echode-engine/src/plugins.rs) は製品のデバイスごとにプロセスを持ち、チェインの各段で次の IPC を投入する。ARCHITECTURE §9.2 のメーカー単位の既定と、§9.3 の「同じヘルパーで続くデバイスは 1 往復」は将来設計である。既存の `SetChain` 試験は異なる実プラグインの製品チェインではない。

**対応案と完了条件**: 同じ隔離単位に連続する実デバイスをヘルパー内で処理する方式を測る。往復・コピー・プロセス数を減らせる候補だが、クラッシュ時の影響範囲は広がる。ユーザーが選べる隔離粒度と、一インスタンス単位の選択肢を維持し、状態、GUI、PDC、再起動の整合を含めて比較する。単なる OS API の置換より大きな変更である。

## 追加の比較候補: Metal 4

製品は従来の `MTLCommandQueue` / `MTLCommandBuffer` を使い、表示ごとにコマンドバッファを作る。macOS 27 専用なら、Metal 4 のコマンドバッファ・allocator の再利用を比較対象へ加えられる。[Apple の説明](https://developer.apple.com/documentation/metal/understanding-the-metal-4-core-api)では、allocator をフレームごとに持ち、GPU 完了後にメモリを再利用できる。[対応の説明](https://developer.apple.com/videos/play/wwdc2025/205/)は M1 以降を含む。

ただし今回の 2D 描画で速くなるかは未測定。M27-04 の可視列と波形列の再生成が支配的なら、API の移行だけでは改善しない。先に CPU の準備時間、encode 時間、GPU 時間、フレーム欠落、確保回数を分けて比較する。Metal 4 ではリソースの保持・residency・同期の扱いも変わるため、単純な名前の置換にしない。

## 維持する判断と、今回採用を勧めない変更

- **GPU をライブの締め切りで待たない**。Unified Memory もリアルタイムの完了保証にはならない。
- **Accelerate の一括導入をしない**。ADR 0058 の M4 の数値は当時の最細列の比較であり、部分列・非有限値・粗い段を含む製品全体の再測定ではない。非 RT の解析、FFT、SRC、オフライン処理は、その実装時に Accelerate / CPU SIMD / Metal と既存のライブラリを同じ条件で比較する。メモリ確保の可能性を理由に、非 RT まで一律に除外する必要はない。RT では API 単位の検査と確保検出・RTSan が必要である。
- **SME、特定世代の命令、120 Hz を全 Mac に固定しない**。最低 OS と CPU / GPU / 画面の能力は別の条件である。配布の基準を M1 から勝手に引き上げない。

  上記は開始時の判断。M1 基準と SME を対象全体の候補にできない前提は、末尾の M4 以降への変更で置き換える。120 Hz と将来世代だけの能力を固定しない判断は維持する。
- **Workgroup の起動時 2 ms ポーリングは、M27-01 より優先しない**。制御側の一度の処理であり、製品の毎ブロックの待ちではない。非推奨 API の置換は公開の代替と測定を伴わせる。
- **APFS clone と IOSurface は既存の対象経路に合わせる**。取り込みはデコード・正準化・ハッシュ検証が必要で、clonefile だけでは置き換わらない。プラグイン GUI / サムネイルは未実装である。
- **AUv3、プラグインウィンドウ、Foundation Models、MIDI などの将来機能を、存在する API だけから完成扱いしない**。今回、それらの新規実装や実機確認は行っていない。

## 実行した確認と限界

- 開始時の `make doctor` / `make verify-plan`: ツール不足なし、既存変更なし。アプリは unknown であり、起動中のアプリを検査した意味ではない。
- 機器を開かない配置 PCM の再現: 上記 M27-03 の結果。性能の閾値や時間計測なし。
- `cargo test --locked --offline -p echode-audio-io --lib clips::tests`: **4 件成功**。ステレオ、SRC、トリム、負オフセット、ループ先頭の保持を含む。名前に `under_rtsan` がある通常テストの実行だけを、新たな RTSan 実行とはしない。
- `plugin_output_is_independent_of_partition_without_a_realtime_deadline`: **1 件成功**。
- `audit_aud07_one_deadline_collects_a_healthy_track_beside_hangs`: **1 件成功**。
- 隔離の最初の実行は実行環境の `shm_open: Operation not permitted` で失敗した。共有メモリを許す環境で同じテストを再実行して成功。最初のログも `target/macos27-audit/partition-test.log` に保持する。これは製品の処理期限の失敗ではない。
- ヘルパーの通常ビルドと Mach-O の確認: 成功、最低 OS の相違を確認。古いバイナリを読んだだけではない。
- `make verify` の最初の `check-verification` は、一時キーチェーンの `SecKeychainCreate` が実行環境の制限で失敗した。ログは `target/macos27-audit/verification-sandbox-failure.log` に保持し、一時キーチェーンを作れる環境で同じ入口を再実行する。最終の合否と再利用は `target/verification/report.md` を正とする。文書だけの変更で、入力が変わらない製品検査は再利用する。

今回未実施: `spike-isolation`、`spike-engine-smoke` / `spike-engine`、`spike-audio`、`spike-draw` / `spike-playhead`、`spike-input` と聴感。実機・アプリ操作の明示依頼がなくユーザー担当であるため。期限超過、消費電力、実画面の滑らかさ、触覚、実出力の聴感は、今回の静的確認やバッファ一致では保証できない。過去の S1〜S3 の数値は更新しない。

## 基盤の保守

status と ARCHITECTURE へ製品とスパイクの差、Rust の最低 OS の相違、現行の隔離粒度を反映し、資料索引とデバッグ手順へこの監査を追加した。PLAN の対象 OS と P1 完了条件は変えていない。製品の挙動・依存・検査入口は変更しないため、新しい恒常検査・Makefile 項目・検査選択条件は追加しない。M27-01 / 02 / 03 の修正時には、それぞれ製品の準備経路、Mach-O の配布契約、PCM の共有と失効の回帰が必要である。

## 追記: 対象を M4 ファミリー以降へ変更（2026-10-17）

監査後のユーザー指定により、対象は **macOS 27.0 以降、M4 ファミリー以降** とする（[ADR 0112](../decisions/0112-m4-minimum.md)）。上記の再現結果や未修正項目は変わらない。

- M27-02 の CPU 最適化の比較基準を、M1 互換から固定した `apple-m4` へ変更する。手元の Rust はこのターゲットを認識し、Xcode の M4 ターゲットは SME / SME2 を定義する。今回、製品のコンパイル設定は変更していない。
- SME / SME2 を利用する既存の実装と Accelerate を、非 RT の解析・バッチ・オフライン DSP の候補へ加える。M1〜M3 の互換性を理由に除外しない。小さな RT ブロックの高速化や確保・ロックの安全性は別に検査する。
- Metal 4 は引き続き比較候補。M4 の Apple9 と M5 の Apple10 の世代差、画面のレート、コア数、メモリ量は区別する。
- Workgroup、PCM の共有、静的描画の再利用の優先度は維持する。今回の前提変更だけで実測による高速化を達成したとは扱わない。

対象の正本と README、設計、status、関連 ADR、監査索引と Canvas を更新した。診断・配布検査の M4 判定は今後の実装範囲として ADR に明記し、検査の選択条件や実行入口は変更しない。新しい実機検査や聴感検査は行っていない。

## 後日対応: 製品接続と再利用（2026-10-17）

[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)に実装と検査の境界をまとめた。

- **M27-01 対応**: 選択デバイスの Workgroup と周期を取得・保持し、製品の初期化・プラグイン追加・再作成・ヘルパー再起動へ渡す。参加前の失敗もヘルパーのループを終了させる。Workgroup だけの変更にも通知から再準備する。機器を開かない公開の AudioWorkInterval を使い、製品の保持と再起動を確認する。実際のデバイスの通知・負荷時の改善量は未確認。
- **M27-02 対応**: 最低 OS を製品・依存・橋渡し検査で macOS 27 に揃え、Rust と C / C++ の依存は固定 M4 を指定する。Swift は SDK 互換の arm64 を維持する。古い最低 OS を環境へ入れた一時 Cargo プロジェクトの IR と Mach-O を確認し、組立前にも arm64 単独・最低 OS 27.0 を要求する。古いバイナリを拒否する検査を追加した。チップ名による起動時の判定と性能改善量は未確認。
- **M27-03 対応**: 同じ素材の変換と配置の結果を共有し、同レートの変換先コピーを省く。監査の 4096 フレーム・ステレオ 2 配置は左右を共有するため、サンプル本体の重複は 65536 bytes から 32768 bytes になる（RSS の実測ではない）。参照中の平面の再利用、変更しない配置の保持、レートの失効、未参照の素材の破棄を検査する。短いトリムだけで全素材を追加常駐させず、その場合の新しい範囲やゲインは必要に応じて読み直す。
- **M27-04 一部対応**: 入力の版とカメラが変わらない間、可視列・波形・ノートと行の配置を再利用する。ピークと再生位置は毎フレーム読む。実文書のトラック追加、表示幅・倍率・カメラ変更で再構成する回帰を追加した。インスタンス配列の組立・波形の連結・アップロードの追加削減は残る。
- **M27-05 / 06、SME、Metal 4**: 条件付きの比較候補として残す。機器・画面・性能の測定値を作らず、並列数や API の新しさだけで採用しない。

Workgroup の無視、旧 OS バイナリの組立、配置間の PCM の複製が失敗になる証拠は、それぞれローカルの `target/macos27-audit/workgroup-before.log`、`deployment-before.log`、`pcm-before.log` に保持する。新しい製品契約 `live-workgroup` / `shared-placement-pcm` / `cached-geometry` は既存の Makefile 入口に登録し、誤りを戻すコピーを拒否する。`.cargo/**` は全自動検査を選択し、結果の共通入力にも含める。最終の実行・合否・見送り理由は `target/verification/report.md` を正とする。

差分確認で、コピー省略の全体共有経路にも従来の非有限値チェックが必要と分かった。正しいハッシュを持つ正準 PCM の末尾へ NaN を含め、全体の配置は拒否し、NaN を含まない短いトリムは再生準備できる回帰を追加した。修正前の失敗は `target/macos27-audit/finite-before.log`。この追加のため最初の全検査は作業側で中断し、修正後に同じ入口を再実行する。

後日の入力検証（2026-10-29）は [ADR 0056 の後続修正](../decisions/0056-audio-import.md#保存済み-pcm-の有限性の検証2026-10-29) を参照する。
上の成功期待は当時の記録として保持し、現在の製品回帰ではトリム外の NaN も保存素材の全読込みで拒否する。
全体共有・コピー後の配置平面の有限性検査は別の単体回帰として維持する。

今回の実機の隔離・音声・実表示・聴感はユーザー担当として見送る。橋渡しと書き出しの時間計測も、今回の接続・共有・所有権の正しさとは分けて未実施とする。新しい依存の版を追加していないため、ネットワークの advisory 更新は行わず、ローカルのライセンス検査を行う。過去の S1〜S3 の測定値を今回の合格へ移さない。

差分確認で、短いトリムだけの素材が複数ある時も、準備の最後まで全素材の一時平面を保持していた。素材の最後の配置の処理時に、配置と共有していない全体の一時平面を解放するようにした。異なる 2 素材の短いトリムで、同時に残る一時素材が 2 から 1 になる回帰を追加した。修正前の失敗は `target/macos27-audit/temporary-memory-before.log`。バイト数や RSS の上限を測ったという意味ではない。

2 回目の全検査は、Swift の `-target-cpu apple-m4` により SDK 内の Testing interface と SwiftShims PCM の機能条件が不一致となり、コンパイルで失敗した（`target/macos27-audit/swift-m4-sdk-failure.log`）。Swift の CPU 指定を取り除き、SDK 互換の arm64 / macOS 27 を維持する。Rust と C 依存の固定 M4 条件は保ち、SDK の検査条件は緩めない。これは性能比較の結果ではなく、このツールチェーンで成立するビルド条件の修正である。

3 回目の全検査では Rust の通常回帰と Swift の製品検査が通ったあと、release の橋渡し生成が proc-macro の `E0463` で止まった。詳細ログでは macOS の `dlopen` が `mis-aligned LINKEDIT string pool` を返した。外部 crate のない一時 cdylib の実読込も失敗し（`target/macos27-audit/release-dylib-before.log`）、release の `strip = "none"` で同じ回帰が通った。最低 OS 27.0 と M4、Rust の最適化レベルは維持する。これは [上流の Rust の既知不具合](https://github.com/rust-lang/rust/issues/157750) と一致するが、手元の固定コンパイラーの実読込を証拠に回避を採用した。全検査を再実行し、合否は検査レポートを参照する。

4 回目の全検査では通常検査が合格し、製品契約の通常テストが全件通過した。負の対照の途中で、独立 cdylib の検査用ソースから OS API の手書き宣言を除くため作業側で停止した。Rust の `std::env::args_os` だけでも、デバッグ情報削除ありの読込失敗と、削除なしの成功を確認し直す。中断した製品契約を合格とせず、最後に正式な入口を再実行する。

5 回目の全検査で、既存の `loop-head-residency` の負の対照が、通常どおり成功して失敗になった。今回の共有する配置平面が保持されるため、旧条件の「準備キーの一致を無効化」だけでは保持を壊せなかった。検査の期待値は維持し、負の対照を準備キャッシュ全体の破棄へ更新した。更新前の検出不足は `target/macos27-audit/loop-contract-before.log` に保持する。

6 回目の全検査は、通常検査、27 の製品契約と27の負の対照、RTSan、入力、全 feature と ASan が通過したあと、TSan の Swift 製品検査で停止した。既存の再生位置の UI 世代更新テストが実 `.play` を送って HAL を開いており、出力バッファの f32 書込みと保存の zlib 書込みが同じアドレスとして報告された（`target/macos27-audit/tsan-hal-before.log`）。UI テストは既存の応答差し替え口で再生要求だけを機器なしにし、位置などの実橋渡しと世代の期待値を維持する。実 HAL の警告は原因未確定として残し、抑制や errno の旧報告との同一視をしない。自動検査の合格をこの警告の解決へ読み替えず、実機の追加調査はユーザーの明示依頼後に行う。

その後の正式な入口では自動検査9項目がすべて合格した。画面なしの `spike-arrange-frame` は9,453行の不一致で止まったが、キャッシュを使わない同プローブの比較点 x=25 pt が既存の M ボタン内にあった。行境界と描画漏れを比較する目的は維持し、左余白 x=2 pt へ移した。同じ161フレーム・50,758行が不一致0になり、境界を1 ptずらした一時コピーの拒否も確認する。旧ログ・画像・JSONは `target/macos27-audit/arrange-probe-before.*`、修正後は `arrange-probe-after.log`、ずらしたコピーは `arrange-probe-shifted.*` に保持する。最終合否は改めて `make verify` のレポートで確認する。

## 後日対応: Swift と全検査を M4 基準へ統一（2026-10-17）

ユーザーから全環境を M4 以降に揃え、汎用設定の方が速い場合だけ例外にするよう指定された。[ADR 0114](../decisions/0114-m4-build-baseline.md) に現在の判断をまとめる。上記の Swift 見送りは当時の記録として保持し、この対応で置き換える。

独立した最小パッケージと新しいキャッシュでも、Swift だけの CPU 指定による不一致を再現した。`-target-cpu apple-m4` と `-Xcc -mcpu=apple-m4` を対にすると、Clang import と Swift Testing が通った。SDK 自体の不具合と確定していたわけではなく、設定の渡し方で解消した。製品・描画・入力の全 Swift ターゲットとテスト、C / C++、直接の swiftc / rustc、Xcode の入口を M4 / macOS 27 に揃える。外部の配布済みシステムライブラリやプラグインの再ビルドを意味しない。

実際の manifest・IR・新しいキャッシュでの import・実テストを基盤の回帰に追加し、共通 env の変更で全結果を失効させる。通常検査と CI は実行ホストの報告された M4 以降 / macOS 27 以降 / arm64 を確認する。リモート CI の機種や実行を確認した意味ではない。CPU 情報の読取りを隔離環境が拒否した場合も未確認として停止する。選択・デバッグ・正本資料も更新する。

既存の橋渡しプローブを同じ M4 向け Rust ライブラリへリンクし、Swift の汎用 arm64 と M4 を交互に各12回比較した。スナップショット1,000回は中央値49.21 / 48.25 μs、10万ノートの100回走査は140.95 / 140.67 ms（汎用 / M4）。同程度で、M4 指定による悪化の証拠は得られなかった。全製品の速度改善へ一般化しない。生データは `target/m4-build-audit/bench/comparison.json` に保持する。正式な `spike-bridge` と全自動検査の合否は `target/verification/report.md` を参照する。実 HAL の TSan 警告は引き続き未確定である。

全ビルド統一後の最初の全検査は、TSan の製品検査で終了済みスレッドと新しいスレッドの errno アクセスが同じアドレスとして報告され、失敗した。同じ nightly・M4 / macOS 27・Apple のランタイムで、製品を含まない Rust 標準ライブラリと呼出しだけの Swift でも同型を再現した。証拠と条件は ADR 0114 に保持し、実 HAL の警告と混同しない。抑制せず正式な入口を再実行し、この警告を解決済みとはしない。

再実行では自動検査9項目、画面なし描画2項目、橋渡しの計測が合格した。基盤89テスト、製品契約27件と負の対照27件も含む。実機・聴感、書出しの時間計測、依存版追加のない advisory 更新は理由付きで見送り、実 HAL と errno の未確定な警告は維持する。最新の合否は `target/verification/report.md` を参照する。
