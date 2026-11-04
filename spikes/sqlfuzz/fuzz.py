#!/usr/bin/env python3
"""usage: python3 -I fuzz.py run|prove|gen [options]   (see README.md)"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sqlfuzz.cli import main
sys.exit(main())
