# ADR 0061: 配布バンドルを生成した Xcode プロジェクトと同じ自己署名で作る

- 日付: 2026-10-13
- 状態: 採用
- 範囲: Phase 1 の配布用バンドル。公証、GitHub Releases、Sparkle、App Sandbox は含まない

## 背景

`make app` はアドホック署名（`codesign --sign -`）だった。アドホックのまま作り直すと、マイクなどの許可が毎回リセットされる（PLAN §8）。開発と配布は同じ自己署名証明書にする（ARCHITECTURE §17）。

ヘルパーはバンドルの無い実行ファイルで、本体の `Contents/MacOS` に並んでいた。許可の確認に名前と署名を出すには、ヘルパー自身の `.app` が要る。library validation を外す権限はヘルパーだけに付ける（監査 PLG-6）。

## 決定

証明書の名前は `Echode Local` である。`make sign-setup` がログインキーチェーンへ一度だけ入れる。秘密鍵のパスワードは作らず、キーチェーンが秘密鍵を持つ。

`project.yml` が Xcode プロジェクトの定義である。`make xcode` が XcodeGen で `Echode.xcodeproj` を生成する。生成物は手で編集せず、リポジトリにも入れない。署名は手動で、識別子は `Echode Local`、App Sandbox は付けない。

`make dist` が release の実行ファイルと XCFramework から `target/dist/Echode.app` を組み立て、同じ証明書で内側から署名する。構成は次のとおり。

- `Contents/MacOS/Echode`
- `Contents/Helpers/EchodePluginHost.app`（`LSUIElement`。library validation を外す権限はこのバンドルだけ）
- `Contents/Frameworks/EchodeFFI.xcframework`

開発用の `make app` は、証明書がログインキーチェーンにあるときは同じ証明書で署名する。無いときは今まで通りアドホックで署名し、ヘルパーは `Contents/MacOS` の隣に置く。配布の `make dist` は証明書が無いと、コンパイルの前に止まる。

本体は、隣にヘルパーがあればそれを使い、無ければ `Contents/Helpers/EchodePluginHost.app` の中を使う。

公証と自動アップデートは Phase 6 のままである。

## 結果

一時キーチェーンの `Echode Local` で署名したバンドルは、アドホックではなく、ヘルパーだけが library validation を外す。画面での操作確認は未実施である。
