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
  選択文字列の生成。
* **`.gro` / `.pdb` 構造ファイルとの対応確認**。
* **XTC トラジェクトリ再生** — 再生 / コマ送り / シーク、FPS 調整、フレーム間の
  補間（スムージング）。
* **表面（ドット）メッシュ表示** — `gmx sasa` の PDB から表示。さらに別ファイルの
  **オーバーレイ表面**を複数、色分けして重ね合わせ可能。
* **ドキュメントレイヤー** — 複数構造を同時に読み込んで比較。各レイヤーは表示・
  識別色・不透明度を個別に持つ。
* **Martini 粗視化ビーズ表示** — 力場の LJ sigma からビーズ半径を、bead type から
  色を決め、本体分子の描画経路で表示。
* **残基名ごとの表示/非表示**、**分子・レイヤーごとの不透明度**。
* **原子選択** — ビュー上でのクリック選択、セレクタ式（例: `aC1 | aC2`）、最短経路の
  「Select Between」、残基名の編集、水素結合を考慮した選択。編集した構造の書き出し。
* **読み込み済みファイルの一覧表示**で、現在ロード中のファイルを一目で把握。

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

MIT License

---

## Author

Yuhei Yamada (Indigo Carmine) 
ORCID: [0009-0003-9780-4135](https://orcid.org/0009-0003-9780-4135)
