## 概要

本アプリケーションは、GROMACS の構造・トポロジー・インデックス・トラジェクトリを
可視化・検証するための GUI ツールです。
もともとは `.top` ファイル内に定義された intermolecular interaction の設定内容を
視覚的に確認し、設定ミスや不整合を検出しやすくすることを目的としていましたが、
現在は MD 構造・粗視化系・表面・トラジェクトリを扱う汎用ビューアへと拡張されています。

対応ファイル:

* `.gro` : coordinate / structure files
* `.pdb` : coordinate / structure files（`gmx sasa` のドット表面を含む）
* `.mol2` : coordinate / structure files

および

* `.top` / `.itp` : topology files（GROMACS の `#include` 展開に対応）
* `.ndx` : index files
* `.xtc` : trajectory files

---

## 主な機能

* **3D 分子ビューア** — ball + stick / ball / wireframe / circles の描画スタイル、
  ドラッグ＆ドロップでのファイル読み込み。
* **GROMACS topology**（`.top` / `.itp`）の読み込みと **intermolecular interaction の可視化**。
* **index (`.ndx`) ファイル** — group の確認、表示切り替え、`make_ndx` コマンド用の
  選択文字列の生成。group 着色の不透明度は構造本体とは別に調整できる。
* **`.gro` / `.pdb` 構造ファイルとの対応確認**。
* **XTC トラジェクトリ再生** — 再生 / コマ送り / シーク、FPS 調整、フレーム間の
  補間（スムージング）。
* **表面（ドット）メッシュ表示** — `gmx sasa` の PDB から表示。さらに別ファイルの
  **オーバーレイ表面**を複数、色分けして重ね合わせ可能。
* **ドキュメントレイヤー** — 複数構造を同時に読み込んで比較。各レイヤーは表示・
  識別色・不透明度を個別に持つ。
* **Martini 粗視化ビーズ表示** — 力場の LJ sigma からビーズ半径を、bead type から
  色を決め、本体分子の描画経路で表示。
* **コンポーネント** — 表示/非表示を切り替えられる名前付きグループ。コマンドバーから
  分割・結合できる（後述）。初期状態は残基名ごとに 1 つ。
* **分子・レイヤーごとの不透明度**。
* **原子選択** — ビュー上でのクリック選択、セレクタ式（例: `aC1 | aC2`）、最短経路の
  「Select Between」、残基名の編集、水素結合を考慮した選択。編集した構造の書き出し。
* **読み込み済みファイルの一覧表示**で、現在ロード中のファイルを一目で把握。

---

## コンポーネントコマンド

**Ctrl+P** でウィンドウ下部のコマンドバーにフォーカスし、`help` と入力すると
アプリ内で構文を確認できます。コンポーネントは*表示上の*グループであり、分割・結合を
行っても PDB / GRO / TOP ファイルは一切書き換わりません。

各原子はちょうど 1 つのコンポーネントに属します。したがって代入は原子の
**所属替え**であり、これが分割と結合を同じ操作にしています。

```
DOM1 = PROT and resid 1-100        # 分割: 該当原子が PROT から DOM1 へ移る
PROT = PROA or PROB or PROC        # 結合: 3 つが空になって消える
```

| コマンド | 意味 |
| --- | --- |
| `NAME = <選択式>` | マッチした原子を `NAME` へ移す（無ければ作成） |
| `<選択式>` | マッチ数を表示するだけ。状態は変えない |
| `show NAME...` / `hide NAME...` | 表示/非表示。`show all` / `hide all` で全体 |
| `only NAME` | `NAME` だけを表示し、他を全て隠す |
| `del NAME...` | 解体。原子は残基名由来のコンポーネントへ戻る |
| `rename OLD NEW`, `list`, `reset`, `help` | |

選択式:

| 要素 | 対象 |
| --- | --- |
| `resname SOL NA` | 残基名 |
| `resid 1-100 205` | 残基番号。複数の数値・範囲を並べられる |
| `index 1-4000` | 原子番号（1 始まり、`.ndx` と同じ） |
| `name CA C1'` / `element C O` | 原子名 / 元素記号 |
| `selected` | 現在ビュー上で選択している原子 |
| `all`, `none` | |
| `sp`, `sp2`, `sp3` | 混成軌道。結合本数から推定 |
| `numbonds 4` | 結合本数。`>=N` `<=N` `>N` `<N` `N-M` も可 |
| `with 2 H` | 隣接水素がちょうど 2 個。カウント指定は上と同じ |
| `and`, `or`, `not` | `&` `\|` `!` でも可。優先順位は `not` > `and` > `or` |
| `( )`, `"NAME"` | 括弧によるグループ化 / 予約語と衝突する名前の引用 |

```
CH2  = element C and sp3 and with 2 H          # メチレン炭素
CH3  = element C and sp3 and with 3 H          # メチル炭素
QUAT = element C and numbonds 4 and with 0 H   # 四級炭素
SITE = selected                                # 現在の選択を切り出す
```

注意すべき点が 2 つあります。

* キーワードなしの語は **コンポーネント名 → 残基名 → 元素記号 → 原子名** の順に
  解決され、解決結果は必ずログに出ます。実際に衝突するのは DNA の残基名 `C`
  （シトシン）で、この順序では炭素より残基名が優先されます。全炭素が欲しい場合は
  `element C` と書いてください。
* `sp`/`sp2`/`sp3`、`numbonds`、`with` は結合情報を参照します。結合は `.top`、
  PDB の `CONECT`、MOL2 からしか得られないため、素の `.gro` では 0 件を返さずに
  「トポロジーが無い」旨のエラーになります。混成軌道は結合本数からの推定なので、
  アミド窒素（隣接 3 本）は `sp3` と判定されます。

---

## 使用目的

以下のような用途を想定しています。

* intermolecular interaction の定義確認
* topology 設定ミスの検出
* index group の整合性確認
* GROMACS シミュレーション前の入力検証
* 構造・表面・粗視化系の視覚的な比較
* MD トラジェクトリの確認

---

## スクリーンショット


---

## ライセンス

GNU Affero General Public License v3.0 以降 (AGPL-3.0-or-later)。
全文は [LICENSE](LICENSE) を参照してください。

---

## Author

Yuhei Yamada (Indigo Carmine) 
ORCID: [0009-0003-9780-4135](https://orcid.org/0009-0003-9780-4135)
