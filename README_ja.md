## 概要

本アプリケーションは、GROMACS のトポロジーファイルおよびインデックスファイルを可視化・検証するための GUI ツールです。
特に、`.top` ファイル内に定義された intermolecular interaction の設定内容を視覚的に確認し、設定ミスや不整合を検出しやすくすることを目的としています。

対応ファイル:

* `.gro` : coordinate/structure files
* `.pdb` : coordinate/structure files
+
* `.top`/`.itp` : topology files
* `.ndx` : index files

---

## 主な機能

* GROMACS topology (`.top`) ファイルの読み込み
* intermolecular interaction の可視化
* index (`.ndx`) ファイルの読み込み
* index group の確認
* `.gro` 構造ファイルとの対応確認
* `make_ndx` コマンドのための選択用文字列の生成
---

## 使用目的

以下のような用途を想定しています。

* intermolecular interaction の定義確認
* topology 設定ミスの検出
* index group の整合性確認
* GROMACS シミュレーション前の入力検証


## スクリーンショット


---

## ライセンス

MIT License

---

## Author

Yuhei Yamada (Indigo Carmine) 
ORCID: [0009-0003-9780-4135](https://orcid.org/0009-0003-9780-4135)