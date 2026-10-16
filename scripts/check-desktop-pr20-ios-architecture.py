#!/usr/bin/env python3
"""Legacy path: run the canonical Desktop-main checker; never pin PR #20."""
import runpy
from pathlib import Path
runpy.run_path(str(Path(__file__).with_name("check-desktop-main-ios-architecture.py")),run_name="__main__")
