#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import sys


MODULE_DIR = pathlib.Path(__file__).resolve().with_name("nginx_compare")
if str(MODULE_DIR) not in sys.path:
    sys.path.insert(0, str(MODULE_DIR))

from main import main


if __name__ == "__main__":
    sys.exit(main())
