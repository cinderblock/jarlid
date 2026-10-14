"""Spike: the distinct words of a lyric file, first spelling kept, one per line.

    keywords.py <plain.txt>

For Whistle's --audio-keywords. Whole lines were tried as phrases too and did worse: the
decoder echoes the phrases back, quote marks and all, at times they weren't sung.
"""
import re, sys

seen, out = set(), []
for word in re.findall(r"[\w']+", open(sys.argv[1], encoding="utf-8").read()):
    if word.lower() not in seen:
        seen.add(word.lower())
        out.append(word)
print("\n".join(out))
