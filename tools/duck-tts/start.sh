#!/bin/bash
# 从任何目录都能起：切到脚本所在目录，模型路径是相对这个目录找的。
cd "$(dirname "$0")" || exit 1
exec .venv-f5/bin/python serve_f5.py
