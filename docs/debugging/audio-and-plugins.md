# デバッグ手順: 音声・再生・プラグイン

音声エンジン・素材・再生と停止・CLAP の隔離とプラグインの窓の症状を切り分ける。
最初の診断と記録する項目は [デバッグ手順](../debugging.md) にある。症状の一覧は `--outline` で見る。

## エンジン・素材・波形

### 原点前のテンポ点で表示と再生位置がずれる・テンポ点の数で換算が遅くなる

保存したテンポ点の整数 tick と分数、出力サンプルレート、問題の tick / sample を記録する。
`TempoMap::flicks_at` / `ticks_at` と、同じ `TempoMap::index` のサンプル換算を比べる。
原点が入力にない場合も時刻ゼロは tick ゼロである。負の部分区間は右端からの積分で丸めるので、左端との差だけで計算し直さない。
`echode-time` の `indexed_conversions_preserve_the_pre_index_model_rounding` は旧モデルの独立計算、
`conversions_search_logarithmically_and_clones_share_the_index` は訪問数と共有を検査する。
製品の照合は engine の `audit_fca13_playback_and_model_agree_before_and_after_zero` と `block_size/tempo.rs` を使う。
モデルの flick は i64、再生索引の内部積分は i128 のため、i64 を超えた位置の拒否と、通常位置の換算不一致を分ける。
保存形式・診断の収集先は変わらない。遅い場合は索引構築と問合せを分け、音声コールバックに構築・確保を移さない。

### 監視の即時破棄回帰が `drop did not return within 3 s` で落ちる

2026-11-02 の W1〜W4 の検査で、`make verify` の `check` がこのテストだけで 8 回落ちた。
標本はいつも `FSEventStreamStart` → `register_with_server` → `mach_msg`（fseventsd への登録待ち）で、大量のビルドの直後に多かった。
そこで 3 秒の期限を破棄だけにかけ、開始は 60 秒の上限（止まった時だけ止める）で区別した。
今は `start did not return within 60 s` なら OS 側の登録待ち、`drop did not return within 3 s` なら破棄のハング（PLG-9 の回帰）である。
以下は変更前の記録で、当時の期限は監視の開始と破棄の両方を含んでいた。
2026-11-02 の SPG-09 検査では全体と単独で失敗した後、同じ期限の単独実行が約 97 秒で完走した。
成功した診断実行の標本は `FolderWatch::start` → `FSEventStreamStart` → `register_with_server` → `mach_msg` であり、破棄の待ちとは限らない。
初回の失敗位置と OS 側の根因は未確定。既修正の run-loop 開始競合と決めつけず、開始・停止のどちらかを標本で区別する。
期限・対象は変更せず、採取ありの診断と正式な検査を区別する。証拠は `target/focused-checks/spg09-watch-timeout.md` と `spg09-watch-recheck/`。
SPG-06 の検査でも単独実行は約 100 秒で完走した一方、全 audit と `make verify` では同じ期限で失敗した。
失敗した全 audit の実行中の標本も開始 RPC の待ちであり、停止処理のハングと断定しない。
記録は `target/focused-checks/spg06-watch-diagnostic.log` と `spg06-watch-sample-52165.txt`。
標本は期限切れの瞬間を特定していないため、OS 側の根因は引き続き未確定である。

### ノートの端から再生すると、その音が鳴り続ける

開始位置がノートの終端ちょうどかを確認する。追いかけ（`Cursor::chase_events`）は終端ちょうどのノートを鳴らさず、
その時刻のノートオフだけを出す（[ADR 0164](../decisions/0164-note-end-chase.md)）。
同じ時刻ではノートオフがノートオンより先に並ぶため、追いかけのノートオンがあると鳴り続ける。
必須契約 `note-end-start` は実ヘルパーで位置移動と停止の復帰を確認する。計画の交換中の場合は `Cursor::take_over` を別に調べる。変わらないトラックは前の声を引き継ぎ、追いかけ直さない（[ADR 0279](../decisions/0279-unchanged-track-voice-handover.md)）。
作り直したトラックは前の声とノート ID で照合し、制御側で置いた位置が使えない時だけ探索で置く。採用のブロックが遅い時は `make spike-scale` の `tempo_adoption`・`marker_adoption` の行と比べる（[ADR 0280](../decisions/0280-placed-cursor-and-voice-matching.md)）。

### 壊れた素材を読むと `attempt to multiply with overflow` / `capacity overflow` で文書が使えなくなる

`media.rs::decode_pcm` のヘッダ長の計算を確認する。
BLAKE3 が一致しても、PCM の形式まで有効とは限らない。
`audit_cur01_malformed_pcm_length_returns_an_error_without_panicking` が、24 バイトの不正ヘッダで再現する。
現行の回帰は debug / release の両方で、サンプル数・バイト数・ヘッダ加算の桁あふれをエラーとして拒否する。
当時の panic の位置は [CUR-01](../reviews/2026-10-25-code-audit.md#cur-01-pcm-のバイト長計算が桁あふれする) を参照する。

### 保存済み素材だけが `audio contains a non-finite sample` になる

ヘッダ長ではなくサンプル値の検証による拒否である。正しいハッシュも値の妥当性を保証しない。
取込み元の復号と保存済み PCM の全読込みを区別し、後者は `canonical-pcm-validation` で再現する。
トリム外の不正値も拒否するが、有限の振幅 1 を超える値は不正としない。原本を自動で書き換えない。

外部 WAV の取込み・一括復号の拒否は `audio-import-validation` で別に検査する。
逐次復号の `AudioFileReader::read` が拒否しても、一括の `decode_file` に同じ検証がある証拠にはならない。
取込み時の拒否はパッケージへの書込み前、保存済み PCM の拒否は読込み時であり、同じエラーでも段階を区別する。

Rust の直接の呼出しは `PackageError::Audio(MediaError::NonFiniteSample)` を返す。
`storage_errors.rs` はヘッダ・長さ・非有限値の拒否の種類、I/O と JSON の `Error::source`、復号の取消とネイティブ状態コードを別々に照合する。
AudioToolbox の失敗は `MediaError` の操作別の変種で、`Property` は番号・状態コード・期待と実際のサイズを持つ。文面から番号を推定しない。
文書のトランザクションの `HostError::Element` はまだ文だけを持つので、これを直接の素材の境界と混同しない。

### クリップゲインの変更で素材の準備に失敗する

保存済み PCM 自体の有限性と、窓のピークにゲインを掛けた結果の桁あふれを区別する。
`clips.rs` の `Window::peak` は配置が読むフレームの未スケールの左右の最大絶対値で、ゲイン変更時には PCM 全体を走査し直さない。
`cached_window_peaks_reject_gain_overflow_without_rereading_pcm` は、極大値を含む窓の拒否、失敗後の準備の保持、極大値がトリム外の場合の受入を確認する。
ゲインとモノの昇格を二重に掛けた疑いは `tests/block_size/clip_gain.rs` の焼込みとの比較で調べる（[ADR 0241](../decisions/0241-unscaled-placement-pcm.md)）。

### 同じ素材の長いトリムを並べるとメモリが素材の何倍にも増える

素材ごとの常駐は、素材と異なる窓の和の小さい方までである（[ADR 0273](../decisions/0273-shared-source-clip-windows.md)）。
`LoadedClip::window` が `Some` の配置は共有の面を読み、`None` の配置は複写した窓か面の全体を読む。
窓の和が素材以上なのに `None` の複写が並ぶなら、`shared_sources` の判定か、前回の窓の種類の照合を調べる。
`clips::tests::audit_ext01_*` は面の `Arc` を 1 回ずつ数えて、素材 1 つ分を超えないことを確かめる。
準備中は素材の全体を復号・変換するので、その一時的な増加は別であり、逐次読込みは未実装である。

### 波形が消える・キャッシュ破損後に表示が変わる

`cache/peaks/<hash>` は派生データであり、段構成・バイト長・有限で順序の正しい列を検査する。
不正なら `Waveforms::load` が正準 PCM から作り直す。再生成できなければ波形なしにするが、原本は変更しない。
`peak-cache-validation` は公開読込みと再生成、`peak_cache_validation.rs` の通常回帰は PCM なしの正常なキャッシュも確認する。
正常なキャッシュで粗い列だけを描けても、PCM の全体が読める証拠にはならない（[ADR 0058](../decisions/0058-waveform-peaks.md)）。
有限の大きなステレオ素材だけが無音の波形になる時は、平均の加算が `f32` へ戻っていないかを確認する。
構築とマップの両方が対象で、`stereo-waveform-average` が極大値・非正規化数と旧キャッシュの再生成を区別する。
`ECHDPK01` は再利用せず PCM から `ECHDPK02` を作るため、素材がない旧キャッシュは波形を出さない。
素材の自動修復や再生音の変更ではない（[ADR 0058 の後続修正](../decisions/0058-waveform-peaks.md)）。
境界が 1 列の途中にある時だけ包絡が違う場合は、`source_span` の素材範囲と `includes_silence` を調べる。
素材前・EOF 後・配置窓の外側を切り詰めても、その無音は列の最小・最大へ含める必要がある。
`waveform-boundary-silence` は正負の包絡・粗い列とマップの端・テンポ変化・キャッシュ再利用を区別する。

### S2 / S3 は通るのに製品だけ負荷に弱い

選択デバイスの Workgroup 取得・ヘルパー参加・変更通知後の再準備を確認する（[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)）。`plugin_missed_blocks` と実機条件を記録する。
トラックが多い時だけ弱いなら、`make spike-plugin-tracks` でコールバック自身の CPU 時間をトラック数ごとに比べる。
トラックあたりの値が数とともに増えれば 2 乗の処理が戻っている。ほぼ一定なら、ヘルパーへの投入のような比例する費用である（[ADR 0236](../decisions/0236-callback-work-per-track.md)）。

### ノートが多い時にループの折り返し・再生中の編集だけ途切れる

`make spike-scale` でノート数を変えた折り返しと、100 / 3,500 ブロック後の採用を比べる（[ADR 0239](../decisions/0239-live-note-cursor-work.md)）。
`wrap_p50_us` と `adoption_p50_us` は音声側、`replace_p50_ms` は制御側であり、混ぜない。
1 トラックの編集で `replace_p50_ms` がノート総数に比例して増えるなら、使い回しが外れている（[ADR 0274](../decisions/0274-reused-track-plans.md)）。
`PlaybackIds` の `planned_notes` と `derived_notes`（テストだけの欄）で、その編集が計画・派生したノート数を数え、`Arc` の同一性かテンポ・レートの変化を調べる。
時間に依らない訪問数は `echode-realtime` の `cursor_tests` で確認する。
プラグインありは `make spike-plugin-tracks` の `plugin-loop` / `plugin-edits` の CPU と実時間の最大値を見る。
この計測は機器なし・期限 1 秒なので、実機の締め切り内という証拠にはしない（UC-47）。
独立した `schedule` の先頭だけが折り返す再現では、出発地点の追いかけを確認する。製品の継続カーソルと違い、前ブロックの集合を持たない。

### 長い楽譜の準備でメモリが増える・ノートの追加ごとに共有領域を交換する

`PlaybackControl::reserved_events()` は親の準備済みイベント件数、`plugin_event_caps()` はトラックごとの最大の共有入力容量を示す。
同じ密度で長さだけを増やして件数が増えるなら、`playback_capacity.rs` の窓と重なりの見積りを調べる（[ADR 0240](../decisions/0240-block-local-event-capacity.md)）。
和音の増加や短いループの周回で増えることは正常で、共有領域は同じヘルパーの寿命では縮めない。
計画の親バッファには、直前の公開計画の声を解放するための一時的な余地も含まれる。
通常の境界は `playback_capacity_tests`、共有領域の密度・拡張・一括削除は `hosted_playback` の `event_capacity` を使う。
`make spike-scale` の `reserved_events` と `replace_p50_ms` は別の問題であり、容量の減少を計画の使い回しの効果と扱わない。
編集を繰り返した後の親の ID 表は `plan_tests::audit_fca19` で確認する。
現在の計画から消えた対応は解放するが、ヘルパーの予約容量・保持中の旧計画・Undo とジャーナルは別の寿命を持つ（[ADR 0267](../decisions/0267-current-playback-id-mappings.md)）。

### フォルダのミュート・ソロと子の音が一致しない

各行の保存値は自身の値であり、親の操作で子のパラメーターは変わらない。
対象と祖先のミュート、対象と祖先のソロ、全体のソロの有無を分けて記録し、`Track::parent` の部分木と照合する。
祖先のミュートは子のソロより優先する。親のソロは子孫を開くが、子だけのソロは兄弟や親自身の素材を開かない。
実効フェーダーの解決は `playback_faders.rs::track_faders`、交換後の補間は `FaderState::carry`、メーターは各トラック自身のフェーダー後を見る。
`cargo test -q -p echode-engine --test block_size folder_mix::` で入れ子・複数ソロ・親の付替え・5 ms の補間・左右・キャッシュを機器なしで照合する。
親の音量・パン・FX が子へ効かないのは、未実装のフォルダバスの境界である（[ADR 0244](../decisions/0244-folder-mute-and-solo-gates.md)、[ADR 0277](../decisions/0277-reaper-style-folder-bus.md)）。
実機だけで起きる段差は、機器・バッファ長・切替順・プラグインと取りこぼし数を UC-49 へ記録する。

## プラグイン

### ヘルパーの保存・復元・パラメーター・窓の要求が時間切れ／busy になる

`plugin main request N timed out; outcome unknown` は実行の取消ではない。
メインスレッドに残る要求が完了するまで後続は busy で拒否し、自動再送しない。
要求番号 N はヘルパー内部の呼出し番号で、ソケットの応答照合の番号とは別である。
原因は文面でなく型で見る。ホストの待ちが先に切れた時は `SessionError::Timeout`、ヘルパーの待ちが先なら `Refused(TimedOut(MainCall(N)))` で、どちらも結果不明である。
後続の拒否は `Refused(Busy)` になる（[ADR 0293](../decisions/0293-typed-helper-boundary-errors.md)）。
プラグイン自身の失敗は `Refused(Plugin(種類))` で、拡張がない（`NoState`・`NoWindow` など）か、プラグインが断った（`SaveRefused`・`LoadRefused`）かを型で見る。
予約済み CLAP ID は `Refused(InvalidParameterId(ID))` で、一括変更の全部が未適用である。
`Refused(UnexpectedMainReply { expected, actual })` はヘルパー内部の手順の不一致で、プラグイン自身の拒否とは分ける。
表示文の解析や自動再送をせず、型の両方の種類を記録し、`main_requests.rs` と `threads.rs` の対応を調べる（[ADR 0309](../decisions/0309-typed-helper-request-failures.md)）。
起動の拒否は `Refused(Launch(原因))` である。未分類の `Other(文)` はなく、旧種類 `0` も拒否する（[ADR 0310](../decisions/0310-typed-plugin-load-boundary.md)）。
`plugin-main-timeout-state` は機器なしの `slow-save` で再現し、`plugin-control-request-ids` は古い応答の破棄を検査する。
完了後の復元バイトと新しい保存結果を比較し、実行中の拒否と、古い応答の取り違えを区別する。
操作と通信の対応・Unicode と状態バイト・拒否理由は `session_operations_tests.rs` で切り分ける。
実装は `session_operations.rs` の `PluginControl` にあり、ホスト経由の保存・復元の別入口はない
（[ADR 0227](../decisions/0227-control-only-plugin-operations.md)）。
親とヘルパーは同じビルドでそろえ、古い起動済みアプリを新しいヘルパーだけに置き換えない
（[ADR 0199](../decisions/0199-owned-helper-control-requests.md)）。

### トラックやマスターが急に無音になり、再生し直すと戻る

自動ミュートを確かめる（[ADR 0282](../decisions/0282-plugin-nan-and-master-auto-mute.md)）。
`AudioState` の `auto_muted_tracks` は NaN・無限大を返した装置と標本、`master_auto_mute` は +18 dBFS を超えた値と標本を示す。
再生の開始で解除し、そのトラックのプラグインをリセットする。書出しでは `plugin_failure` が `PluginBlockFailureReason::NonFinite` を返し、表示は `returned a NaN or an infinity` になる。

### 音源なしのノート・Sine Pad が鳴らない

音源のないトラックは無音である。デバイス一覧に `dev.echode.sine-pad` があるかを確認する。
一覧にない時は `make bundled-plugins` とヘルパー隣の `plugins/`、アプリ内の `Contents/PlugIns/`、同梱用走査キャッシュを確認する。
`host event capacity was not prepared` はイベント領域の準備漏れであり、音声側の確保や固定発音数で回避しない
（[ADR 0123](../decisions/0123-explicit-sine-pad-plugin.md)）。
音色・声の継続だけが違う時は `echode-dsp/src/sine_pad_tests.rs`、隔離後の経路は `hosted_playback` を使う。
DSP の状態と CLAP の保存ペイロードは別の形式なので、どちらの入口で差が出たかを記録する（[ADR 0266](../decisions/0266-host-independent-sine-pad-dsp.md)）。

### 同梱 CLAP の初回だけ RTSan が malloc を検出する

子プロセスの標準エラーを収集し、`_tlv_get_addr` と `ForbidAlloc` が並ぶかを確認する。
dylib 固有の検査用 TLS は、ヘルパーの最初の非 RT 起動段階の `start_processing` で初期化する。
検知を外さず、未発音の新しいヘルパーで最初の処理も検査する。

### CLAP が鳴らない・走査失敗

プラグイン ID、バンドル走査（macOS の `.clap` ディレクトリを含む）、キャッシュ、ヘルパー起動、入出力バスを追う。走査失敗時は子の終了コードとシグナルを確認する。本体プロセスにプラグインをロードして切り分けてはいけない。
`clap-scan.jsonl` に `"rejected":true` の行があれば、ヘルパーが拒んだ版で、版が変わるまで読み込まない。
行がなければ、まだ答えがない（走査中・時間切れ・ヘルパーを起動できない）。時間切れのバンドルは 5 秒の後に 120 秒で走査し直すので、最大 2 分ほど遅れて一覧に出る。
2 回とも時間切れなら次の起動まで出ない。`echode-plugin-host --scan バンドル` を `time` で測り、125 秒を超えるかを確かめる（[ADR 0285](../decisions/0285-scan-timeouts-are-not-verdicts.md)）。
FFI の目録内部の失敗は `crates/echode-ffi/src/catalog.rs` の `CatalogError` で区別する。
`MissingHost` は指定されたパス（指定なしは `None`）、`Catalog` は元の `echode_plugin_host::CatalogError`、`Json` は元の JSON の原因を持つ。
C ABI は診断の文を返さず従来の `-1` に畳むので、内部の回帰は `cargo test -q -p echode-ffi --lib catalog::tests` で確認する。
起動の拒否は `SessionError::Refused(Refusal::Launch(...))`（stderr があれば `SessionError::Launch` の内側）で読む。
`LaunchFailure.kind` が読込み・作成・起動の原因で、I/O の errno・NUL の位置・CLAP の版も保持する。`diagnostic` は表示用で、分類に使わない。
対応は `echode-plugin-host/src/launch.rs`、通信は `echode-sandbox/src/launch_failure.rs`、実ヘルパーの回帰は `cargo test -q -p echode-plugin-host --test isolation launch_` である。
ヘルパー内部では `HostError::Load(LoadError)` の `source` から clack の原因を読める。
実行中のソケット・共有領域の失敗を起動拒否へ変換する入口はない。
未分類の拒否の旧種類 `0` は `InvalidData` で拒否するので、アプリとヘルパーの版も照合する（[ADR 0310](../decisions/0310-typed-plugin-load-boundary.md)）。
走査先は子プロセスの `ECHODE_CLAP_ROOTS` に閉じ、実際の機器・インストール済みのプラグインを使わない。
2026-10-28 の ⌘D 変更の通常検査では、`audit_rea08_descendant_pipe_does_not_extend_deadline` の最初の JSON 出力が 2 秒の期限切れとなった。
失敗は子孫のパイプ保持ケースではなく、最初の短い shell の起動・完了待ちで、単独実行では再現しなかった。原因は未確定である。
個別のログは `target/focused-checks/clip-duplicate-scan/`。期限・対象を変更せず、正式な `make verify` の結果と区別する。
ミュート／ソロの一括操作の検査で再発し、期限切れの子は `running`・両出力 0 byte で、採取では `_dyld_start` にとどまっていた。
さらに、従来の子孫保持ケースが shell の起動待ちだけで合格し得たため、専用のネイティブ検査ヘルパーへ変更した。
製品の期限は維持し、出力済み・直接の子の終了も検査する。期限切れの PID・終了状態・byte 数・パイプの開閉で、起動前と保持を区別する
（[ADR 0190](../decisions/0190-native-scan-deadline-fixtures.md)）。OS 側の起動遅延の根因は未確定である。
2026-10-30 の検査コスト改定でも、ネイティブ fixture の最初の正常 JSON が同じ 2 秒の期限切れとなり、単独でも一度再現した。
子は `running`・両出力 0 byte で、直後の同じバイナリの直接起動は約 8 ms、同じ Cargo 構成の再実行は約 0.45 秒で完了した。
shell 除去でこの環境の起動遅延自体が解決したとは扱わず、期限は維持する。
初回の失敗は検査時間履歴と作業ログへ残し、再実行の合否で過去の失敗を上書きしない。

### ヘルパーの応答が `window title is not text` / `parameter text is not UTF-8` で切れる

制御ソケットの文字列が不正であり、音声 PCM の非有限値やプラグイン自身の拒否とは別である。
`SessionError::Io` の内側の `io::Error::get_ref` を `ControlTextError` に downcast する。
`WindowTitle` と `Parameter` が文脈で、元の `Utf8Error` は変種の中と `source` から読める。
`valid_up_to` は各文字列内のバイト位置、`error_len == None` は未完の UTF-8 である。
元の不正なバイトは診断へ残さない。アプリとヘルパーの版を照合し、自動再送や文字列の置換で隠さない。
実ソケットの全経路は `cargo test -q -p echode-sandbox --lib control::text_tests`、
公開操作は `session::operations_tests::parameter_operation_retains_invalid_utf8_from_the_helper` で切り分ける。
制御ソケットの fuzz の種も同じ入口を通す（[ADR 0312](../decisions/0312-typed-control-text-errors.md)）。

### ヘルパーの応答が `launch failure is malformed` で切れる

ローダーの正当な拒否（`SessionError::Refused(Refusal::Launch(...))`）ではなく、通信の起動拒否ペイロードが壊れている。
`SessionError::Io` の内側の `io::Error::get_ref` を `LaunchDecodeError` に downcast する。
起動時の stderr があれば、外側の `SessionError::Launch.error` の内側を調べる。
`Truncated` は `LaunchField` と必要・残りのバイト数、`UnknownKind` と `InvalidErrnoFlag` は受けた値を持つ。
`Diagnostic` は元の `Utf8Error` を変種と `source` に保持し、位置は診断文の先頭から数える。
診断文や壊れたバイトの解析では分類せず、アプリとヘルパーの版・送信側を確認する。自動再送はしない。
実ソケットの回帰は `cargo test -q -p echode-sandbox --lib control::launch_tests`、
公開操作は `session::operations_tests::parameter_operation_preserves_malformed_launch_refusal_causes` である（[ADR 0313](../decisions/0313-typed-launch-refusal-decoding.md)）。

### ヘルパーの応答が `plugin failure is malformed` / `refusal ...` で切れる

正当なプラグインの拒否ではなく、通信の拒否ペイロード自体の構造が不正である。
`SessionError::Io` の `io::Error::get_ref` を `RefusalDecodeError` に downcast する。
`WrongSize` は `RefusalField` と必要・実際のバイト数を持ち、プラグインのペイロードはその種類も持つ。
欠けた種類、未知の種類・期待／実際の応答コード、余分なバイト、不正なポートの向きは別の変種である。
長さは当該フィールドだけを数える。生バイトや診断文から分類せず、アプリとヘルパーの版・送信側を確認する。自動再送はしない。
回帰は `cargo test -q -p echode-sandbox --lib refusal_tests` で、実ソケットの全拒否経路と公開操作を通す。
起動拒否の不正は前節の `LaunchDecodeError` のままである（[ADR 0314](../decisions/0314-typed-refusal-structure-errors.md)）。

### ヘルパーの応答が `chain message is the wrong size` / `port description ...` / `unknown control message` で切れる

拒否ペイロードではなく、外側の制御メタデータが不正である。
`SessionError::Io` の `io::Error::get_ref` を `ControlDecodeError` に downcast する。
`WrongSize` は `ControlField` と必要・実際のバイト数で、ポートの列と数は `input` で方向を分ける。
固定長の `expected` は一致すべき長さ、不足した列の `expected` は最低限必要な長さである。
`MessageLength` は宣言長と許容範囲、未知の種類・コマンドは受けた値、余分なポートの列は残りの長さを持つ。
文面や生バイトで分類せず、アプリとヘルパーの版・送信側を確認する。自動再送はしない。
回帰は `cargo test -q -p echode-sandbox --lib decode_tests` で、実ソケットと公開操作を通す。
パラメーター・窓・通知の内側の構造は次の節で扱う（[ADR 0315](../decisions/0315-typed-control-metadata-errors.md)）。

### ヘルパーの応答が `parameter message ...` / `window report ...` / `relay ...` で切れる

外側のメタデータは正しく、パラメーター・通知・窓の報告の中身が不正である。`parameter value is not finite` と `unknown parameter text reply` もここに入る。
`SessionError::Io` の `io::Error::get_ref` を `PayloadDecodeError` に downcast する。
`Truncated` は `ParamPayload` と `ParamField`、列の中なら 0 始まりの項目の番号、必要なバイト数と残りを持つ。
文字列の本体（`Name`・`Module`・`Text`）の `expected` は宣言された長さなので、巨大な値は直前の長さのフィールドの破損を疑う。
`TrailingBytes` は余りの長さ、有限でない値は ID と IEEE-754 のビット、未知のコードとフラグは受けた値を持つ。
`InvalidWindowFlag` は通信の順で最初の不正なフラグだけを返す。
同じ文の `SessionError::NonFiniteParameter` は送信前の引数の拒否で、I/O の原因を持たない。
文字列の不正な UTF-8 は `ControlTextError` で、この型にはならない。
文面や生バイトで分類せず、アプリとヘルパーの版・送信側を確認する。自動再送はしない。
回帰は `cargo test -q -p echode-sandbox --lib payload_tests` で、実ソケットと公開操作を通す（[ADR 0316](../decisions/0316-typed-parameter-payload-errors.md)）。

### 共有メモリの準備が `shm_open`・`shm_unlink`・`ftruncate` で止まる

`RegionError::SharedMemory` の `operation` と `error.raw_os_error()` を確認する。
`SessionError::Region` でも元の変種を保持し、`RegionError::source` も元の `io::Error` を返す。
`Open` は作成の権限・記述子・同名の衝突、`Unlink` は名前の除去、`Truncate` はサイズ変更の失敗である。
起動の拒否（`Refusal::Launch`）や実行中の時間切れと混同せず、診断文から errno を推測しない。
回帰は `cargo test -q -p echode-sandbox --lib region::error_tests` で、機器は開かない。
実 `ftruncate` のサイズ境界、同名の作成、表示と source、成功時の名前の除去と FD の属性を照合する。
macOS は共有メモリの長さをページ単位に切り上げるため、要求値との完全一致を仮定しない（[ADR 0311](../decisions/0311-typed-shared-memory-errors.md)）。

### プラグインが多いとヘルパーが `Too many open files (os error 24)` で起動しない

親はヘルパー 1 つにつき記述子を 4 つ持ち、Finder から起動したアプリの soft の上限は 256 である（`launchctl limit maxfiles`）。
最初のヘルパーの前に上限を上げるので、この文が出るなら上げられなかったか、hard の上限が低い（[ADR 0237](../decisions/0237-helper-descriptor-limit.md)）。
そのプロセスの上限を `launchctl limit maxfiles` と、可能なら `lsof -p <pid> | wc -l` で記録する。回帰は `cargo test -p echode-plugin-host --test descriptor_limit` である。

### ヘルパーとの接続が記述子の規則の違反か `control message parts were interleaved` で切れる

ヘルパーとの接続が `a control message carried more than one file descriptor`・`control data was truncated`・`a second file descriptor arrived before the first was consumed` で切れたなら、相手が記述子の規則を破っている。
本体はその読込みの記述子を全部閉じて、そのソケットを使わなくなる。本体の記述子が増え続けるなら、`lsof -p <pid>` で種類を分け、`plugin-control-descriptors` の契約が通るかを確かめる（[ADR 0288](../decisions/0288-received-descriptors-are-all-owned.md)）。
どの規則かは `io::Error::get_ref` を `DescriptorRuleError` に downcast して読み、2 個以上なら届いた数も持つ。文面で分類しない。
回帰は `cargo test -q -p echode-sandbox --lib rights_tests send_tests` である（[ADR 0317](../decisions/0317-typed-control-transport-errors.md)）。
`control message parts were interleaved` は、64 MiB を超えて `PART` の枠に分けたメッセージの途中に別の要求の枠が来た。
`ControlDecodeError::InterleavedPart` の `expected` と `actual` で両方の要求 ID を読む。送り手は 1 つのスレッドが送るので、送り手の側で同じソケットに書く経路が増えていないかを疑う。
大きな状態のプラグインで保存が `Timeout` になるなら、同期の取得は応答を 2 秒しか待たず、約 1 GiB/s では 2 GiB 前後が上限である。
背景の保存が遅いなら、`try_recv_tagged` が読める分を続けて読むことと、送受信の領域が 1 MiB になっていることを確かめる。
回帰は `cargo test -q -p echode-plugin-host --test isolation a_state_longer_than_one_frame` と `control_send_tests.rs` である（[ADR 0318](../decisions/0318-split-control-messages.md)）。

### ヘルパーへの要求が `control message was cut off at the deadline` などの送信の期限で失敗する

本体の要求は、送信と応答の待ちで 1 つの期限を共有する。原因は `io::Error::get_ref` を `ControlSendError` に downcast して読み、文面で分類しない。

- `control message was not sent before the deadline`（`Unsent`）: ヘルパーが読まず、1 バイトも渡せなかった。流れは保たれ、次の要求は送れる。
- `control message was cut off at the deadline`（`Abandoned`）: 途中で切れた。`sent` と `total` で、どこまで渡ったかを読む。本体は送る側を閉じ、ヘルパーは終わりを読んで止まる。
- `control socket stopped sending after a message was cut off`（`Closed`）: 前の要求が途中で切れた後の要求で、何も送っていない。

どれも要求は実行されていない（`SessionError::Timeout` の結果不明とは違う）。ヘルパーの制御スレッドが読まない理由を先に疑う。
処理スレッドが止まったプラグインでの `CancelRemap` と再起動の待ちは期限がない。`sample <helper pid>` で制御スレッドが `handle_control` か `control_thread` の待ちにいるかを確かめる。
両端が同時に長いメッセージを送って止まるなら、`wait_for_room` が `POLLIN` で `Socket::fill` を呼んでいるかと、記述子が `O_NONBLOCK` かを確かめる（Darwin は送信の `MSG_DONTWAIT` を無視する）。
回帰は `cargo test -q -p echode-sandbox --lib wait_tests operations_tests` と、`cargo test -q -p echode-plugin-host --test isolation a_large_restore_crosses a_restore_behind_a_cancel` である（[ADR 0319](../decisions/0319-control-send-deadline.md)）。

### バイパスの表示と音が一致しない・解除後に古い音が鳴る

文書のデバイス ID と `bypassed`、計画の要求・適用版、プラグイン失敗、ドライ／ウェットの平面数を確認する。
バイパスした音源ではノートが無音でも同じトラックの配置音声は鳴る。解除時のリセットと現在のノート追跡を確認する（[ADR 0120](../decisions/0120-device-bypass.md)）。

### GUI の書出しが始まらない・文書を切り替えると内容が違う

同梱 `Contents/MacOS/echode-cli` の欠落は `Export CLI is missing from the app bundle` になる。`make app` で同版をそろえる。
大文字小文字を区別しないボリュームで `Echode` と衝突しないよう、CLI のバンドル名は `echode-cli` である。
下部の「詳細」から元のパス・出力先・オプション・終了コード・`exported` の行を保存する。自動収集はユーザー文書・専用スナップショットを含めない。
2026-11-05 に、TSan のロード証拠が大量の標準エラーになり、先頭 64 KiB だけの表示から末尾の `exported` が落ちることを確認した。
現行の読込みは先頭・末尾各 32 KiB と省略表示を残す。成功判定は終了コードから行い、文字列の存在で成功を作らない。
`export-diagnostics-ui` は同じ `ExportJob.render` に大量の診断を渡し、末尾と表示予算を照合する（[ADR 0268](../decisions/0268-bounded-export-diagnostics.md)）。
準備で止まる場合は進行中のジェスチャー、参照する `media/objects` の欠落と BLAKE3 不一致、専用ディレクトリの作成失敗を区別する。
文書ワーカーはコピー中に次の要求を待たせる。準備後の CLI は独立したパッケージを開き、元の文書のロックを取り直さない。
`export-snapshot-ui` は PCM のコピー後に元の文書を編集・終了・削除し、次の文書を開いたまま実 CLI を完走させる。
前回設定の記憶と移行は、下記の「書出し設定が移動先で戻らない・保存の警告が出る」で切り分ける。
途中取消は専用の制御パイプで要求する。アプリ終了・強制終了の後の一時物は再開せず、ユーザーの同意なしに探して削除しない（[ADR 0256](../decisions/0256-export-cancellation.md)）。

### 書出しの数値欄を変更しても開始できない・前の値が使われる

数値欄は `ExportSettingsDraft` が文字列で保持し、`WorkspaceExportView.validatedSettings` を開始ボタンと保存パネルの入口で共有する。
空欄・後ろの単位・桁区切り・非有限値・範囲外は `export.validation` に理由を出し、前の有効値を採用しない。
余韻とピーク目標の小数点は表示ロケールに従う。無効欄は無視するが、再度有効にすれば下書きを検証し直す。
読込み待ちは `Loading export settings`、入力不正はその下の理由、CLI 起動後の失敗は下部の「詳細」で区別する。
`export-input-ui` は非表示の実シートのフィールドエディターを使い、修正後の引数と設定未保存を検査する。
`ExportSettingsDraftTests` はロケール・u32 の丸め境界を検査する。実画面・VoiceOver は UC-59 に残す（[ADR 0262](../decisions/0262-export-numeric-drafts.md)）。
ユーザーが入力した文字列を診断の自動収集へ追加しない。

### 書出し後の Finder 表示が出ない・別のファイルが選ばれる

「Finder で表示」は CLI が正常終了し、後片付けが終わった時だけ現れる。失敗・取消・出力先の拒否では前回の出力を案内しない。
「詳細」の `Output:` と `ExportJob.completedOutput`、ボタンの `export.reveal` を照合する。
文書の現在の URL や保存パネルで選んだリンクを再解決せず、完了ジョブが保持する固定パスを `NSWorkspace` へ渡す。
`ExportRevealTests` は実 CLI と表示先の代役で検査し、Finder 自体は開かない。実画面・VoiceOver は UC-58 に残す。
完了後に利用者がファイルを移動・削除した場合やディレクトリ自体を置換した場合は追跡しない（[ADR 0261](../decisions/0261-reveal-export-output.md)）。
ログの収集先は変えず、ユーザー文書や出力ファイルを自動収集しない。

### GUI の書出し先がフォルダのリンク切替後に変わる

GUI の CLI は専用スナップショットだけを入力にするため、CLI 側の検証だけでは元文書のパッケージを保護できない。
`ExportJob.start` が最初の待機より前に親を解決し、葉を保持した固定パスを `render` へ渡しているか確認する。
「詳細」の `Output:` は固定したパスを示す。選択したリンクの現在の参照先と異なっても、開始後のリンク切替には追従しない。
元パッケージ内の外向きリンクも、そのリンク自体を置換するので拒否する。外にある外向きリンクはリンク自身だけを置換し、参照先は保つ。
`ExportDestinationTests` と `export-destination-ui` は実 CLI・機器なしでこの境界を確認する（[ADR 0260](../decisions/0260-pinned-gui-export-destination.md)）。
ディレクトリ自体の並行移動・置換を防ぐサンドボックスではない。ユーザー文書やスナップショットを自動収集しない。

### CLI が `choose an output outside the project package` で止まる

出力先が元パッケージ自身・内部のファイル・内部を指すリンクなら、復旧や発音を始めず拒否する。
新規 WAV も外へ出す。元／出力のパスと、親フォルダ・既存ファイルのリンクを確認し、外の出力先を指定し直す。
`cannot resolve export output directory` は親がないか解決できない場合で、フォルダを自動作成しない。
`export_destination.rs` は実 CLI・相対パス・リンク・全原本のバイトを照合する。
失敗した要求に `exported` や完了の進捗があれば、そのログを保全する。ユーザーのパッケージは自動収集しない
（[ADR 0259](../decisions/0259-export-destination-boundary.md)）。

### 書出しプリセットが保存・適用できない・一覧に出ない

名前付きプリセットは `workspace.export.presets.v1` の JSON 版 1 で、前回値や共通既定値とは別である。
一覧で選ぶだけでは適用せず、「適用」で現在の入力欄だけを変える。別の Mac やプロジェクトのコピー先には移さない。
同名の保存は拒否する。名前の前後の空白と Unicode の正規化後に比べ、大文字・小文字は区別する。
未知の版・不正な設定・壊れた保存物は警告して保持し、空の一覧として上書き・修復しない。
この状態でも手動入力・既定値・書出しは使える。削除済みの選択は適用時に再照合し、古い表示から復活させない。
読込み中・保存パネル中・文書交換後の操作は無効である。削除は現在の欄・前回値・共通値・Undo を変えない。
`ExportPresetTests`・`ExportPresetStorageTests` と `export-presets-ui` で保存の持続・入力・拒否を切り分ける。
実操作と VO は UC-61 に残す。プリセット名や設定値を診断へ自動収集しない（[ADR 0264](../decisions/0264-named-export-presets.md)）。
この非表示シートの AX ツリー探索では追加前後とも要素を得られなかった。存在の回帰は展開後のネイティブ入力欄を通し、AX の合否は実画面の確認と分ける。

### 共通の書出し既定設定が使われない・前回値が変わらない

パッケージの前回値と旧パス設定を、この Mac の共通値より優先する。共通値のキーは `workspace.export.default.v1`、JSON の版は 1 である。
欠落・未知の版・不正値では出力のレート（未準備なら 48 kHz）のアプリ初期値へ戻す。読むだけでは共通値を修復しない。
「既定設定を使う」は現在の欄だけに適用し、「既定設定として保存」は検証済みの欄だけを共通値へ保存する。
「既定設定を初期化」は共通値と現在の欄を初期化するが、パッケージや旧パス設定は消さない。
読込み中・保存パネル中・文書交換後の旧シートからの操作は受け付けない。書出し開始・前回値の保存・Undo は別の操作である。
`ExportDefaultsTests` と `export-defaults-ui` で欄からコマンドへの引渡し・別プロジェクト・優先順を調べる。
実クリック・読み上げと配置は UC-60 に残す。既定設定やユーザー文書を診断へ自動収集しない（[ADR 0263](../decisions/0263-common-export-defaults.md)）。

### 書出し設定が移動先で戻らない・保存の警告が出る

`export.json` の版 1 と設定値、旧 UserDefaults の `workspace.export.v1.<正規化したパス>` を区別する。
正常なパッケージ内の設定を優先し、欠落・未知の版・不正値・16 KiB 超過では旧値、共通既定値、初期値の順に戻す。読取りだけでは修復しない。
旧版の設定は次の書出しの準備が成功した時に移す。それ以前に移動したパッケージは旧パスの値を持たない。
「詳細」の `Export settings were not saved in the package` は、書出し自体の成功／失敗とは別の警告である。
パッケージの書込み権限・`export.json` と旧一時名のリンクや種別・復旧への同意を調べる。リンク先を削除して直さない。
`ExportSettingsTests`・履歴の `tests/export_settings.rs`・`RecoveryConsentTests` で移行と拒否を確認する。
再現には元／移動先の区別・設定の版・警告を残し、ユーザー文書や設定を debug-report へ自動収集しない（[ADR 0257](../decisions/0257-portable-export-settings.md)）。

### ノート終端の回帰だけが `control.status().playing` で止まる

2026-11-04 の書出し取消の検査で、`hosted_playback` 全体の `note_edges` がこのアサーションで 2 回失敗した。
単独実行と、状態・ブロック長・チェイン・rewind・`plugin_failure()` の診断を加えた後の反復では再現していない。根因は未確定で、診断の追加を製品の修正とは扱わない。
失敗の完全ログは `target/export-cancel-hosted-recheck/2.log`、診断後の実行は `target/export-cancel-hosted-diagnostic/` と `target/export-cancel-hosted-repeat/`。
CLI の取消はこのテストから呼ばれない。再発時は拡張したアサーションの `playing`・`failed`・適用済みのコマンドと計画・位置・ヘルパーの失敗を残す。
同じビルドの単独と全体を比較し、無視・期限延長・アサーション削除で通さない。正式な合否は最新の `target/verification/report.md` で区別する。
2026-11-05 の必須契約でも再発し、64 フレーム・チェイン 1・Locate 後の状態は `playing: false, sample: 48000, failed: false, applied_command: 2` で、ヘルパーの失敗・ミスはなかった。
今回の経路は、live cleanup の受領待ちで時計が保持される間に、回帰が決め打ちのコールバック数を使い切っていた。
`publish_status` は cleanup 中に `playing` を false として公開する。無音の callback 数を実際に再生したフレーム数として数えない。
回帰はサンプル位置が必要量進むまで全 callback の無音を照合し、cleanup 中だけ検査ドライバーを休ませる。5 秒の hung-helper guard と、失敗・ミスを許さない条件を維持する。
初回のログは `target/focused-checks/note-end-cleanup-first-failure/` に保持する。

### 書出しの取消中が続く・取消を押しても完了になる

`ExportJob.cancelRequested` は要求であって成功の証拠ではない。取消の制御は stdout でなく stdin の EOF で、進捗が閉じても poll を続ける。
準備のコピー・復号・初期化・ファイル同期や、最大 60 秒のヘルパーブロック待ちは区切りまで続く。強制終了や自動再送で代用しない。
「詳細」の終了コード 130 と `export cancelled` は公開前の取消、0 と `exported` は公開が先に成立した完了である。
他の終了コード・公開後の耐久性不明を取消へ書き換えず、原本や完成ファイルを無条件に削除しない。
`ExportCancellationTests` と CLI の `tests/export_cancellation.rs` で、閉じた stdout・文書交換・原本と一時物を切り分ける。
制御する Rust テストで `Child::wait` を使う時は、先に `child.stdin.take()` のハンドルを保持する。`wait` 自身が stdin を閉じると意図せず取り消す。
再現には要求時の準備／処理／仕上げの表示・終了コード・詳細を残す。音声・専用パッケージは debug-report へ自動収集しない。

2026-11-11 の `make verify` の `check-swift` で、`contractCancellationSurvivesClosedProgressAndDocumentExchange` が一度だけ `Cancellable export did not deliver live progress` で失敗した。
CLI へ引き継ぐ前の最初の照合で、制御ソケットの変更の経路は通らない。単独の 3 回、全体の 3 回、CPU の負荷を並べた全体の 5 回では再現しなかった。
当時の文は固定で、ジョブが早く終わった（失敗）のか、進捗が 10 秒届かなかったのかを分けられなかった。
今はその時点の `running`・`fraction`・`message`・`detail` と経過時間を出す。再発したらこの値で、準備（`freezeExport`）の失敗か、子の起動か、進捗の読込みかを分ける。
当時のログは `target/focused-checks/export-cancel-flake/` に保持する。期限を延ばして隠さない。

### 書出しの進捗が出ない・99% で止まる

準備中は素材のコピーやプラグイン初期化で総長が未確定のため、不定の進捗になる。
CLI の `--progress-json` の stdout と stderr を別々に保存し、`version: 1` と改行で完了したレコードを確認する。
stdout をパイプへ渡す観測者は実行中に排出する。GUI は `ExportJob.render` で排出し、不正・版違い・後退した通知は無視する。
99% は全フレームの処理後もノーマライズの後段変換・ファイルの flush・同期・公開が残っている状態で、`complete` 通知だけでも成功にしない。
最終的には下部の詳細の終了コードと診断を確認する。同期失敗で完成ファイルが残る場合は、再送や削除を自動化しない。
CLI が終了したのに表示が残る場合は、子孫が stdout を引き継いでも EOF を待ち続けていないか確認する。
子が UI 通知の間に終了しても、既に pipe にある末尾の通知は排出する。
終了後の読取りは 64 KiB までとし、子孫が stdout へ書き続けてもジョブを保持しない。
`ExportProgressExitTests` が UI 通知を保留したまま子を終了させ、この境界を決定的に再現する。
`ExportProgressTests` の `export-progress-ui` が終了前の受信と記述子の引継ぎを検査する（[ADR 0255](../decisions/0255-export-progress.md)）。
2026-11-04 の数値入力の変更中に、通常の並列 Swift 全体で 50% の待機だけが 1 回失敗した。
入力と進捗の組合せ、および同じ全体の再実行では再現しておらず、原因は未確定である。
初回の抜粋は `target/focused-checks/export-progress-first-failure.log`、切分けは `target/focused-checks/export-input-progress/` に残す。
このアサーションには `running`・率・メッセージ・詳細を追加した。再発時は準備中・起動失敗・受信待ちを区別し、期限・アサーション・並列性を緩めない。
2026-11-07 のテンポ差分生成の変更時にも通常の並列 Swift 全体で再発し、`running=true`・率 `nil`・`message=Exporting audio`・詳細は空だった。
同じ入口の次の実行では再現せず、期限・判定・並列性と書出しの製品コードは変更していない。原因は引き続き未確定である。
今回の抜粋は `target/tempo-diff-verification-retry.log` に残す。再試行の成功で初回の失敗を取り消さない。

### 書出しの回帰が直列実行だけで `waitUntilExit` に止まる

2026-11-04 の `check-real-hal` は HAL の呼出しではなく、新しい `ExportProgressExitTests` の終了待ちで 300 秒の期限を超えた。
イベント列はそのテストの開始を示す一方、通常ログの末尾は出力のバッファで前のテストに留まることがある。
`sample` の `ExportJob.render` → `NSConcreteTask waitUntilExit` を確認する。証拠は `target/export-settings-hal-timeout.sample`。
終了後の pipe の排出は上限付きで行い、poll が終了を確認した後に Foundation の run loop 待ちへ入り直さない。
回帰は `ExportProgressExitTests` を `--no-parallel` でも実行する。HAL の防護を無効にしたり、期限を延ばしたりして通さない。

### float WAV を読み手が拒否する・ヘッダ変更後に音声の比較がずれる

RIFF 範囲の float の `fmt` は形式コード 3・18 バイト・`cbSize=0`、`fact` は 4 バイトのフレーム数である。
`audit_wav` は RIFF のサイズと空・短い・複数ブロックの書き出しを調べる。固定の 44 バイトを飛ばして読むと、追加したメタデータを音声と誤認する。
`tests/support/mod.rs::chunk` で `data` を取り出し、コミット済み基準の全バイトと比較する。差がコンテナだけなら `make bless` は不要。
再現時はフレーム数・サンプルレート・読み手の名称とエラーを残し、音声の差と形式の拒否を分ける（[ADR 0243](../decisions/0243-float-wav-metadata.md)）。

RIFF 範囲の `--format pcm24` の `fmt` は 40 バイト・形式コード `0xfffe`・24 bit・PCM subtype で、`fact` はない。
RIFF 範囲の `--format pcm16` は 16 バイトの `fmt`・形式コード 1・16 bit で、拡張と `fact` はない。
`tests/pcm16.rs`・`tests/pcm24.rs` は実 CLI の形式と独立した AudioToolbox の読込み、float との差を比較する。
PCM の飽和と半 LSB の丸めは仕様で、既定のディザはなし。非有限値の拒否・上限・出力保全は `src/wav_tests.rs` も確認する。
形式が違う出力を float として比較せず、再現には `--format` も残す（[ADR 0248](../decisions/0248-pcm16-wav-export.md)）。
単体テストで正常系が `export durability is unknown` になり、故障系が予想外に成功する場合は、`arm_wav_fault` の共有を確認する。
故障はプロセス全体に効くため、`wav_tests.rs` / `rf64_tests.rs` / `dither_tests.rs` の全ファイル書込みは同じ `WRITER` mutex を取る。

### ノーマライズの音量が違う・`normalization scratch I/O failed` で止まる

`--normalize-peak-db DB` は余韻込みの左右最大サンプルから共通ゲインを決める。LUFS やトゥルーピークではない。
無音を増幅せず、ディザは後で加える。PCM の 0 dBFS では正側が飽和し、ディザ後の標本には丸めの差も出る。
比較には `--format`・`--dither`・目標・長さと `exported` の版を残し、同じ設定の無効時の WAV と `data` を比較する。
一時保存は出力と同じフォルダで、最終 WAV に加えてステレオ float の 1 フレーム 8 バイトを使う。空き容量と権限を先に確認する。
`normalization scratch I/O failed` は一時保存・再読込みの失敗で、元の出力は置き換えない。99% の間にはこの再読込みと量子化も含む。
一時音声は開いた直後にリンクを外し、終了時に破棄する。診断収集へ音声を追加しない。
`normalization_tests.rs` と `tests/export_normalization.rs` が分割・TPDF・非有限値・読取り失敗・原本保持・隔離音源を検査する。
GUI の設定引渡しは `ExportNormalizationTests`。実画面・VO・聴感は UC-57（[ADR 0258](../decisions/0258-peak-export-normalization.md)）。

### ディザ付きの整数 WAV が毎回違う・無音がゼロでない・float と誤差が増える

`--dither tpdf` は 16 / 24 bit PCM の量子化直前に ±1 LSB の三角分布を加える。無音も整数 ±1 / 0 になり、非クリップ領域の差は最大 1.5 LSB になる。
`none`（既定）の半 LSB と混同しない。float との併用は、プロジェクトや出力を開く前に拒否する。
同じ版と入力なら再書出しは一致する。差がある場合は `--format`・`--dither`・フレーム数・`exported` の版と Cargo.lock を保存する。
`src/dither_tests.rs` は分布・分数 LSB の誤差・左右の相関・分割不変性・故障注入、`tests/dither.rs` は実 CLI と独立した復号を検査する。
`make spike-render` は float と整数 PCM・ディザ有無の release 書込みを計測する。実機の聴感は UC-52（[ADR 0251](../decisions/0251-tpdf-export-dither.md)）。

### 大きな WAV が RF64 になる・読み手が長さを誤認する

メタデータ込みの RIFF 長が u32 を超えると、全形式が RF64 に自動切替する。拡張子は変えない。
先頭の `ds64` に 64 bit の RIFF 長・データ長・フレーム数を置き、32 bit の RIFF 長と `data` 長は `0xffffffff` とする。
`fmt` は全形式で 40 バイトの WAVEFORMATEXTENSIBLE、float の `fact` は `0xffffffff` とし、実フレーム数は `ds64` を使う。
古い読み手が RF64 に対応しない時は、RIFF の上限内まで長さを短くして形式の拒否と音声の差を区別する。
`src/rf64_tests.rs` は 4 GiB 超の疎ファイルを `AudioFileReader` で開き、先頭・中間・末尾の左右と EOF を検査する。
`tests/rf64.rs` は CLI の最初のブロック後に書込み失敗を入れ、巨大な出力なしで原本と `.part` の除去を確認する。
大容量の拒否を期待した古いテストをそのまま走らせると、RF64 では数 GiB の実書込みになる。長さだけの拒否は期待せず、故障注入か疎ファイルへ置き換える。
容量計算は u64 だが、レンダラーのフレーム数は `u32::MAX` のままである（[ADR 0249](../decisions/0249-rf64-wav-export.md)）。

### 書出しの余韻が短い・`infinite plugin tail` / `export length overflowed` で止まる

長さの省略時だけ初期化時の CLAP tail を使う。`--tail-frames N` はその値へ加算せず、出力レートの N フレームで置き換える。
`--frames N` は曲末からの長さではなく総フレーム数なので、短い値では曲も途中で切れる。両オプションは併用できない。
CLAP の `>= i32::MAX` の申告値は無限として明示指定を求めるが、明示した tail の値は無限と解釈しない。
曲末との和が `u32::MAX` を超える場合は RF64 でも拒否し、既存の出力は保つ。ヘッダの byte rate エラーとは分ける。
再現には指定したレートと両オプションの有無、復旧した `exported` の版を残す。
`tests/tail.rs` は実 CLI と隔離 Sine Pad の自動・短縮・延長・ゼロ、空の曲と配置音声、復旧した編集の曲末、入力の拒否を検査する。
無限の申告値と明示値の区別・加算の上限は `tail_tests` で検査する（[ADR 0250](../decisions/0250-explicit-export-tail.md)）。

### オフラインの FX が読み込まれる・素材まで無音になる・書出しに失敗する

文書の `offline`、`bypassed`、`state` を分け、保存形式 5 と旧版からの変換を確認する。
`fx-offline-ui` は実ボタンと履歴、`plugin-offline` は隔離グラフと記録済み状態の復元、`offline-export` は実 CLI のログである。
`chain_of` と `project_plugin_ids` / `routes_with_saved_state` の対象、計画の要求・適用版を確認する。
`device rendering requires the isolated plugin host` が全オフライン時に出たら、ホストなし書出しの対象判定を確認する。
再有効化後の未検出・状態オブジェクトの読込エラーとは区別する（[ADR 0125](../decisions/0125-device-offline.md)）。

### CLI の失敗が文だけで分類できない

Rust の呼出し元では `echode_cli::CliError` の変種と `Error::source` を調べる。
I/O は `CliIoOperation` とパス・元の errno、引数は `ArgumentError` の入力値・構文と範囲の拒否を分ける。
保存・API・素材・再生の原因は元の型のままで、文を解析して分類しない。
ブロック拒否は `PluginBlockFailure` の装置 ID・CLAP ID・`reason` を確認する。
`DurabilityUnknown` は rename 後なので完了ファイルを削除・自動再試行せず、`Cancelled` と区別する。
`tests/typed_errors.rs`・`error_tests.rs`・`normalization_tests.rs` と隔離の `auto_mute`・`deadline_fade` が診断と保全を検査する。
標準エラー・終了コード・進捗の形式と収集範囲は変わらない（[ADR 0305](../decisions/0305-typed-cli-errors.md)）。

### プラグインを使う書き出しが `did not finish before the deadline` で止まる・並列の書き出しで `missing plugin`

書き出しは実時間の期限を使わず、1 ブロック 60 秒だけ待つ。この文が出たら、ヘルパーが応答しなくなったか、1 ブロックに 60 秒以上かかっている。
テスト用 CLAP の故障 `slow-process` で遅いプラグインを再現する。`missing plugin` は走査キャッシュの読み違いで、
キャッシュは置換で書くので切り詰められた内容は見えない。走査の出力が 64 MiB を超えたバンドルは `larger than` で走査を止める（[ADR 0231](../decisions/0231-offline-export-and-scan-cache.md)）。

### プラグインの遅延表示が更新されない・音がずれる

値は出力レートのサンプル数で、PDC はまだ行わない。値が正しく表示されても他トラックと音がずれるのは未補正の制約である。
未取得・オフライン・一覧取得失敗は値を隠し、拡張または `get` がないプラグインは 0 と報告する。
`device.parameters` の `latency_samples` と、活動列の同名フィールドを比べる。
初回が 0 のままなら `threads.rs::query_latency` と `Ports`、再起動後だけ古ければ `PluginRestarted` の受信と親の保持を調べる。
親の値が新しければ、Swift の文書 ID・デバイス ID と `DeviceParameterState` を調べる。
`hosted_playback/latency.rs` は 256 → 512 → 0 を別プロセスで報告させる。`PluginLatencyTests` は機器なしで表示と旧文書の拒否を確認する。
第三者の設定・出力レート・報告値・再起動前後を記録し、補正済みとは扱わない（[ADR 0246](../decisions/0246-plugin-latency-reporting.md)、UC-51）。

### パラメーター一覧が出ない・値が音に反映しない・Undo で音が戻らない

デバイスの列の表示文が境界を示す。`the project has no running plugin` と `the device has no running plugin` は文書・オフライン・未検出、
`no output device is available` は停止中の出力起動、`parameter changes timed out` はヘルパーの処理スレッドの `flush` を疑う。
`document thread stopped` は文書を閉じた後の要求か、ワーカーの異常終了の後の要求、`device request panicked` はデバイスレーンでの異常終了である。
`document thread panicked` は文書の手番の panic で、以後の要求は `stopped` になる。stderr の `echode-worker` の panic が原因を示す（[ADR 0204](../decisions/0204-document-service.md)）。
完了が返らない要求は `completion_tests.rs`、`failed to join thread: Resource deadlock avoided` は `document-lane-completion` で切り分ける。
`echode-ffi` のテストが `removal index ... should be < len` と `panic in a function that cannot unwind` で abort したら、完了コンテキストの寿命を疑う。
コールバックの `send` が走っている間に待つ側が抜けられるので、コンテキストはスタックに置かず、コールバックが `Box` で所有する。
一覧取得の失敗は列の「パラメーターを再読込み」で対象だけを再試行する。検索・選択・履歴は変えず、再失敗も同じ列に出す。
プラグインの再起動や状態の再読込とは異なるため、クラッシュしたヘルパーや未検出のプラグインの復旧を保証しない。
再試行で他の列も消える・連打で要求が重なる場合は `device-list-retry-ui` と `DeviceParameterRetryTests` を使う
（[ADR 0140](../decisions/0140-device-parameter-list-retry.md)）。
`plugin-params` は実ヘルパーとエンジン、`fx-params-ui` は画面と履歴のログである。
Undo では記録のハッシュとヘルパーが最後に保存・読込したバイトを比べ、一致すれば読み込まない（[ADR 0130](../decisions/0130-clap-parameter-editing.md)）。

## 再生・停止・ループ

### 再生開始が遅い・準備中に停止できない

`pending-transport`（UI 受付）、`queued-stop`（ABI 投入時取消）、`transport-cancellation`（HAL 初回発音禁止）の 3 境界を切り分ける（[ADR 0097](../decisions/0097-cancellable-retained-transport.md)）。

### トランスポートや試聴が受理されたのに反映されない

`PlaybackControl::command_revision` と `PlaybackStatus::applied_command` で、制御側の受付とコールバックの適用を分ける。
`BufferTooSmall` で拒否された要求は受領番号を進めず、自動再送もしない。
命令や試聴集合の返却が滞る時は `PlaybackControl::service` の継続と、`playback.rs` の往路・復路の空きを調べる。
ループは命令リングではなく計画の交換なので `pending_plan` と準備エラーを確認する。
単一の `PreviewFader` も制御側で検証済みの集合へ変換し、音声側は `QueuedCommand::Faders` だけを受け取る。
回帰は `playback_command_tests.rs`、試聴の PCM・確定計画待ち・分割長は既存の `mixer-group-audio` で切り分ける。

### 再生が `audio callback panicked` で止まる・アプリが理由を残さず落ちる

エラーの `<場所>: <文>` は、panic した Rust のソースの位置である。そのブロックからコールバックは無音で、プレイヤーを再び使わない（[ADR 0238](../decisions/0238-audio-callback-panics.md)）。
アプリの記録は `~/Library/Logs/Echode/panic.ndjson` で、`make debug-report` が `app-logs/` へ写す。
`source` が `output callback` / `audition callback` ならコールバック、`thread` ならほかのスレッドで、`backtrace` が付く。FFI の中の panic は、この行を書いた後に異常終了する。
再現は、その場所を通る入力の `cargo test` で行う。デバッグのテストの `GuardAlloc` は panic 中の確保を許すので、`ForbidAlloc` の内側の panic もテストの失敗として文が出る。
理由のない SIGABRT は、panic ではない確保を `GuardAlloc` が止めたことを疑う。

### 再生中のぶつぶつ・停止時のクリック

`audio.status` の `plugin_missed_blocks` と `pending_plan` を確認する。
定期保存確認による一時バイパス排除（[ADR 0101](../decisions/0101-live-output-discontinuities.md)）、音声側の排他グラフ所有（[ADR 0102](../decisions/0102-exclusive-audio-graph.md)）、5 ms の停止波形減衰（[ADR 0103](../decisions/0103-live-stop-waveform.md)）、待機しない後片付け（[ADR 0105](../decisions/0105-nonblocking-stop-cleanup.md)）のどの境界かを確認する。
[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md) 以降は素材をプラグインの手前で減衰し、マスターの停止フェードは使わない。
`stop_fade_offset` は素材の減衰位置である。停止後も余韻・自発的な発音が出ることは正常で、停止中の `plugin_missed_blocks` も実際の処理失敗を表す。

### プラグインの取りこぼしで段差が出る・復帰後も無音になる

`plugin_missed_blocks` の増加と `plugin_failure()` のデバイス・理由を先に見る。フェードしても取りこぼしは成功にならない。
アプリでは `app-logs/echode.ndjson` の `echode_engine::audio` の行が、取りこぼしたデバイス・トラック・CLAP ID・理由・位置と、締め切りを越えたブロックを残す（[ADR 0322](../decisions/0322-product-log.md)）。
`plugin-deadline-fade` は実ヘルパーの 2 回目の処理を `process-wait:` のソケットで保留し、対象トラックだけの 5 ms の減衰・復帰を PCM で照合する。
この故障はテスト専用であり、実機を開かず過去の期限を渡す。`process reached` / `process release` の失敗はソケットの到達・解放を調べる。
計画交換の後だけ段差が出る時は `Prepared::carry` の `plugin_declick` の引継ぎ、左右が変わる時は失敗した段でなく最終段のバス形を確認する。
復帰後も失敗回数が増えるなら補間の問題ではなく、ヘルパーの終了・保留・再起動要求を切り分ける。
ヘルパーが遅くないのに小さいバッファでだけ取りこぼすなら、停止の診断の `deadline_ns` を見る。入口の遅れと機器の安全余白（`OutputDevice::safety_frames`）で待ちが縮む（[ADR 0283](../decisions/0283-output-time-callback-deadline.md)）。
`block_size/plugin_deadline.rs` は途中の反転と連続失敗を標本数で照合する。実機の聴感は UC-50 に残す
（[ADR 0245](../decisions/0245-plugin-deadline-fades.md)）。

### 停止中にプラグインの時間・鍵盤の発音が止まる、または余韻が切れる

`playing = false` とライブ処理の停止を混同しない。保持した出力は無音入力の処理を続ける。
`stopped-plugin-output` で実ヘルパーと後段 FX の出力、`stopped-plugin-tails` でノートオフと自然な余韻の終端を確認する。
音源・FX の `bypassed` / `offline`、出力が既に開いているか、位置移動・準備取消でリセットが要求されていないかを分ける。
停止だけで `cleanup_pending` が立つなら旧仕様か不具合を疑う。位置移動の後片付け中なら正常である。
位置移動直後の停止で期限切れになる場合は `stop-cleanup` を使い、保留中に通常の素材処理を始めていないか確認する。

### 停止で開始位置へ戻らない・戻る時に余韻が切れる・開き直すと開始位置が消える

`audio.status` の `tick`（現在位置）、`start`（記憶した開始位置）、`paused` を分けて読む（[ADR 0155](../decisions/0155-remembered-playback-start.md)）。
停止は `Rewind` で時計だけを戻す。余韻が切れるなら停止の経路が `Locate`（リセットあり）になっていないかを `stop-return-tails` と `playback-start` で切り分ける。
開始位置はパッケージ直下の `transport.json` にあり、履歴・Undo・復帰の対象外である。
壊れた・新しい版・リンクのファイルは 0 として読むので、開始位置が 0 に戻る時はこのファイルの中身と種類を確認する。

### ループ範囲のドラッグで再生位置が動く・終端以降から勝手に戻る

押下では開始位置を送らず、離した時のクリックだけが `transport.set_start` を送る。
ドラッグの範囲はオフ中は下書きだけ、オン中は `transport.loop` を 1 回送る（[ADR 0156](../decisions/0156-ruler-loop-range-and-entry.md)）。
`piano-ruler-loop-ui`・`arrange-ruler-ui`・`arrange-ruler-cancel-ui` で入力と取消、`loop-entry` で音声側を切り分ける。
範囲欄のフォーカス移動・オンオフで端点が変わる場合は、`ProjectController.loopRange` と `LoopEntry.range` を比較する。
下書きと送信は整数で共有し、未変更の表示文字列を拍から再変換しない。
`integer-loop-draft-ui` は Double の精度外の端点も製品要求まで保持することを検査する。
手入力した欄の不正値・ゼロ長は拒否し、反対端を丸めたり左右を交換したりしない
（[ADR 0201](../decisions/0201-integer-loop-drafts.md)）。
既存端の調整は上端 8 pt のグリップから行う。画面外の端を、帯の切れ目でつかむことはできない。
片端だけの調整にならない場合は `RulerHandles.hit` と `RulerPress.edge`、つかんだ範囲とポインターのずれを確認する。
別エディタ・範囲欄・オンオフの変更で下書きが消えるのは、古い解放による上書きを防ぐ正常な取消である。
`loop-handles-ui` と `loop-handles-cancel-ui`、各オフスクリーンプローブで入力・取消・描画を分ける。
アレンジの行だけがずれる場合は `arrangeTop` を描画とヒットから二重に足したり除いたりしていないかを確認する。
`spike-arrange-frame` の `rulerMismatches` は帯と印の画素不一致、`maxHeaderLaneError` は上端補正後の左右の誤差である。
`audio.status` のループがオンでも、終端以降へ置いた再生は通常進行する。
ブロック処理で終端に着いた場合は次のブロックで戻るため、同じ値の `tick` でも位置移動と混同しない。

### 空白への移動・再開で以前の残響が鳴る

停止・シーク時の `active.reset()`、`NOTE_CHOKE`、宣言された全 MIDI ポートへの CC120 / All Notes Off を確認し、自然な余韻と区別する（[ADR 0099](../decisions/0099-transport-plugin-state.md)）。
ライブ停止では余韻を残す。上の消去は位置移動・未発音の準備取消とオフライン停止の検査であり、ライブ停止へ戻さない
（[ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)）。

### 左右の消失・定位変化・ループやフェーダーの段差

逆位相・異振幅の左右信号で素材読込・エフェクト間・フェーダー後を照合する（`stereo-media`、`stereo-plugin-chain`、`audio-loop-seam`、`audio-fader-ramp`、[ADR 0095](../decisions/0095-product-audio-buses-and-continuity.md)）。

### ループ変更での再読込・トリム位置ずれ

`clips.rs` の保持キーと `CachedTrack` の失効条件を確認する（[ADR 0108](../decisions/0108-phase1-loop-readiness.md)）。内容位置 0 は `start - offset` のテンポ積分で、負のオフセットは無音となる（[ADR 0109](../decisions/0109-placement-bounds-and-plugin-capture.md)）。

### 長時間・短時間の実機音声計測の失敗

処理期限超過（`late`）と HAL 呼出欠落（`gaps`、`pmset -g log` の Sleep / Wake や `/usr/bin/log show` の coreaudiod デバイス変更）を区別する（[ADR 0096](../decisions/0096-measurement-completion.md)）。短時間確認は `--native-rate` を使う。

## プラグインの窓

### 窓アイコンが消えない・閉じた窓が再び開として表示される

アイコンは物理的な可視性ではなく寿命を示す。別アプリの操作中に隠れていても開の表示は正しい。
`plugin-window-state` でソケット受信順の `window_state`、`plugin-window-status-ui` で画面の `openPluginWindows` を確認する。
活動の `window` は編集時の過去の文脈、`closed` は未読期間内の閉鎖の有無であり、現在の表示へ代用しない。
開く要求の返答から楽観的に表示を更新していないか、未読圧縮で最後の寿命を保っているかも調べる。
削除・オフライン後の残留は `publish`、文書交換後の残留は `DocumentSession.retire` と、処理が保持するセッションを確認する
（[ADR 0138](../decisions/0138-plugin-window-status.md)）。

### プラグイン付きトラックの選択が止まる・パラメーター画面が重い

2026-10-20 のユーザー報告は「選択のたび」と「汎用パラメーター画面」の 2 症状であり、独自プラグイン GUI の問題ではない。
機器なしで、旧欄の全行一括生成（800 個で約 0.68 秒）と再選択での作り直し、文書ワーカー上の応答待ちを再現し、対応した
（[ADR 0131](../decisions/0131-device-view-and-responsive-parameters.md)）。
再発時は、プラグイン名・版・パラメータ数、再生状態、下部の表示、初回と再選択、停止時間、入力全体が固まるか操作が無効表示になるかを区別する。
初回だけ一覧が遅いなら、音声状態の `opening` を見る。真の間に編集・Undo が終わるなら、出力の準備（ヘルパー起動・素材）の時間である。
未取得の列の「音声出力を準備中」は同じ `opening` の表示であり、個別プラグインの進捗率ではない。
出力の準備が終わっても一覧が来なければ「パラメーターを読み込み中」へ戻る。失敗とオフラインは待機表示より優先する。
状態と列の表示が食い違う場合は `device-preparation-ui` と `DevicePreparationTests` を使う（[ADR 0141](../decisions/0141-device-output-preparation-status.md)）。
同テストの期限失敗は、未保留デバイスの返答・初回の音声状態・準備後の音声状態のどの待ちかをメッセージで区別する。
サニタイザーの失敗ログを保持して単独実行と全体実行を比べ、期限だけを延長して製品不良や並行実行の影響を隠さない。
`opening` の間に編集も終わらないなら、ワーカーが準備を待っている。`output-opening-worker` を実行して手番を確かめる。
導入後に位置・ループ・追加したトラックが反映されないなら `output-opening` で導入時の差分を確かめる（[ADR 0136](../decisions/0136-output-opening-off-worker.md)）。
開いた出力で FX を追加した時は `pending_plan` を見る。新しいヘルパーの初期化と共有領域の準備は `echode-graph-prepare` が待つ。
その間の編集と停止は `background-graph`、導入後のデバイス要求の待ちは Session の機器なし回帰で区別する。
状態の復元やループ変更の後で既存ヘルパーが再生成されるなら、投入時の状態一致の保持と次の `Pending` の経路を確認する。
`opening` が偽でも `pending_plan` は真になり得る。置換専用の列の表示は未接続である。
導入時の差分のグラフも `echode-graph-prepare` で作る。準備中に編集した後の再生が `playing` のまま `running` にならないなら、その計画の公開待ち（`holding_start`）である。
導入直後にワーカーが待つ・置き換えられる計画が鳴るなら `output-install-graph` で導入の順序と保留を確かめる（[ADR 0333](../decisions/0333-install-diff-graph-off-worker.md)）。
再生中の編集の素材も `echode-media-prepare` で読み、その間は前の計画が鳴る。`running` が真で `pending_plan` が真なら読込みの残りである。
テンポの変更の後に拍が跳ぶ・戻るなら `playing-edit-media` で採用時の `retime` と、位置移動の `LocateTick` を確かめる。
再生中に準備した出力の導入で、置き換えられる計画が一瞬鳴るなら `output-install-held-play` を使う（[ADR 0330](../decisions/0330-playing-edit-media-and-tempo-retime.md)）。
記録済みのプラグイン状態は名前（保管庫の BLAKE3）で比べ、新しいヘルパーと状態の変わったヘルパーだけが `echode-graph-prepare` で読む。
無関係な編集が状態を読む・ヘルパーを作り直すなら `plugin-state-names` で比べ方を確かめる。
読めない状態は `PlaybackError::State` のデバイスと原因（`PackageError`）で分かる（[ADR 0329](../decisions/0329-plugin-states-by-name.md)）。
機器なしの出力は計画の採用に検査側の `pump` が必要である。初回の準備だけを待つ時に `Session.opening()` の全置換待ちを使わない。
製品アプリでは開いた直後の `opening` は事前準備（`audio.prepare`）である。最初の再生がまだ待つなら、押した時点の状態を分ける。
`opening` が真なら事前準備の途中、偽で `pending_plan` が真なら停止中の編集の素材読込み（`echode-media-prepare`）の残りである。
`playing` が真で `running` が偽のまま止まるなら、読込みの失敗か保留の解除を `output-prewarm-play` で確かめる。
開いても `opening` にならないなら、`output-prepare-on-open` で controller の送信、`output-prepare-worker` で C ABI を確かめる
（[ADR 0147](../decisions/0147-output-prewarm-and-background-media.md)）。
毎回止まるなら、`DeviceParameterListView.visibleRows` の数と `table.numberOfRows` を比べ、全行を作っていないかを確認する。
列が作り直されているなら `DeviceStripView` の列の保持（文書の切替でだけ捨てる）を確認する。
値の往復で止まるなら、`device-lane-held` を実行し、背景保存の確認と適用済み状態の読取りが通信中の制御端点を待っていないか確かめる。
画面だけが古い一覧へ戻るなら、`device-view-slow-ui` で要求世代の照合を確かめる。
テスト用 CLAP の `ECHODE_TEST_FAULT=params-wait:<socket>` は一覧をメインスレッドで保留し、機器なしで遅いプラグインを再現する。
実測時間を使う性能検査は通常回帰から分ける。実機操作とサンプリングが必要な場合は担当・許可を確認し、既存アプリの終了・置換は行わない。
[追加受入条件](../phase2.md#p2-02-デバイスビューとパラメーター編集着手中)を参照する。

### プラグイン GUI がすぐ消える・ループ切替で状態が戻る・Space が届かない

`plugin-window-visibility` は OS の一時非表示を閉鎖と誤判定する経路を確認する。
`NSPanel.close` の記録、親・自分・同じ親のヘルパーの最前面判定を調べ、実際の閉鎖と隠れただけの状態を区別する。
`plugin-window-loop` は状態の取り込み後に未保存の値を変え、停止中・再生中のループ切替で同じ GUI と値が残ることを確認する。
`Pending.restore_states` がループだけで真にならないこと、保留中のプロジェクト編集の復元を落とさないことを調べる。
キーは `plugin-key-events`（AppKit）、`plugin-key-socket`（制御通知）、`plugin-key-ui`（既存コマンド）で境界を分ける。
ヘルパーはコマンドを名前（`PluginCommand::name`）で送り、画面は `WorkspaceCommand` の `rawValue` で受ける。
コマンド名を変えて片方だけ直すと、中継したキーは黙って無視される。`plugin-key-ui` は未知の名前をその場で失敗にする。
以前はここで来ない音声コマンドを待ち続け、Swift の全体実行が 1 件のテストで止まった（[ADR 0155](../decisions/0155-remembered-playback-start.md)）。
文字入力中は転送しない。独自入力欄は `NSTextInputClient.selectedRange` の公開状態を確認する。
実画面の再発時はプラグイン名・版、前面のアプリ、窓を開く前の状態保存、入力欄の編集中かも記録する
（[ADR 0134](../decisions/0134-plugin-window-lifetime-and-shortcuts.md)）。

### プラグインのウィンドウが開かない・フォーカスを奪う・値や Undo が届かない

エラー表示で境界を分ける。`the plugin has no window` はプラグインが `clap.gui` を持たない、
`the plugin has no Cocoa window` は Cocoa の API を持たない、`the plugin refused the window` は `set_parent` の拒否である。
`plugin window timed out` はヘルパーのメインスレッドが 8 秒以内に `create` / `show` を終えなかったことを示す（[ADR 0132](../decisions/0132-helper-plugin-windows.md)）。
開くまでの待ちは、停止中の最初の表示で出力とヘルパーを準備する待ち（`opening`）と区別する。
ウィンドウがキーになる・隠れない・戻らない場合は、記録用の窓で規則を確かめる `plugin-window-helper` を先に実行し、
合格なら AppKit の実画面の問題としてユーザーの確認へ回す。実画面の操作は依頼がある時だけ行う。
値が一覧へ届かないなら `plugin-window-relay` で処理スレッドのリングと制御スレッドの読み戻しを、
Undo が増えない・増えすぎるなら `plugin-window-undo-ui` でジェスチャーの数え方と静止の待ちを確かめる。
窓が閉じている時のプラグイン自身の変化は自動保存だけで、Undo に入らないのが正しい。
窓での変更が無関係な編集の後に戻る・窓が閉じるなら、活動の `window` と `dirty`、ワーカーの保留（`PluginSaves::held`）、
セッションへ渡した `keep_live_plugin_edits` の集合、置換の経路の状態がヘルパーの適用済みのバイトかを順に確かめる。
実ヘルパーでは `plugin_gui::audit_doc_a2_...`、ワーカーでは `saves::tests` を実行する（[ADR 0229](../decisions/0229-plugin-window-live-edits.md)）。
テスト用 CLAP の `ECHODE_TEST_FAULT=gui:<socket>` は、ヘルパーに記録用の窓を選ばせ、ソケットからプラグイン画面の操作を送る。
