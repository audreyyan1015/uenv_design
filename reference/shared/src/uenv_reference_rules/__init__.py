"""Python ports of the inspected Rust text scorers' rules.

These follow inspected rules with one explicit correction: unknown semantic
LaTeX commands must not be erased to manufacture a match (sqrt(33) != 33).
They are not a claim of official evaluator parity; full corpus comparison is
a migration gate. No candidate string is evaluated as Python code.
"""
import math
import re
from uenv.sdk import binary_score


def _take_braced(text):
    depth = 1
    for index, character in enumerate(text):
        depth += (character == "{") - (character == "}")
        if depth == 0:
            return text[:index], text[index + 1:]
    raise ValueError("Unbalanced braces")


def _numeric(text):
    try:
        text = text.strip()
        for command in ("\\dfrac{", "\\tfrac{", "\\frac{"):
            while command in text:
                start = text.index(command)
                numerator, rest = _take_braced(text[start + len(command):])
                if not rest.startswith("{"):
                    return None
                denominator, tail = _take_braced(rest[1:])
                text = text[:start] + numerator.strip() + "/" + denominator.strip() + tail
        cleaned = ""
        for character in text:
            if character in "0123456789./-":
                cleaned += character
            elif character == "+" and not cleaned:
                pass
            elif character in ",$ \t\n\r":
                pass
            else:
                return None
        if "/" in cleaned:
            numerator, denominator = cleaned.split("/", 1)
            value = float(numerator) / float(denominator)
        else:
            value = float(cleaned)
        return value if math.isfinite(value) else None
    except (ValueError, ZeroDivisionError, OverflowError):
        return None


def numeric_equivalent(left, right):
    a, b = _numeric(left), _numeric(right)
    if a is None or b is None:
        return None
    return abs(a-b) <= 1e-9 * max(abs(a), abs(b), 1.0)


def extract_boxed(text):
    last = None
    index = 0
    marker = "\\boxed{"
    while index < len(text):
        if text.startswith(marker, index) and (index == 0 or text[index-1] != "\\"):
            start = index + len(marker)
            depth = 1
            end = start
            while end < len(text):
                if text[end-1] != "\\":
                    depth += (text[end] == "{") - (text[end] == "}")
                if depth == 0:
                    last = text[start:end]
                    index = end + 1
                    break
                end += 1
            else:
                index = start
        else:
            index += 1
    return last


def _assignment(text):
    if "=" in text:
        left, right = text.split("=", 1)
        if re.fullmatch(r"[A-Za-z0-9_]{1,3}", left.strip()) and right.strip():
            return right.strip()
    return text


def _math_normalize(text):
    text = re.sub(r"\\([A-Za-z]*)", lambda m: "*" if m[1] in {"frac", "cdot", "times"} else "", text)
    return "".join(c for c in text if c.isascii() and (c.isalnum() or c in ".+-*/[](),^")).lower()


def score_gsm8k(answer, private_data):
    target = private_data["answer"].strip()
    predicted = answer.rsplit("####", 1)[-1].strip()
    if not target or not predicted:
        return binary_score(False)
    if answer.strip() == target or predicted == target:
        return binary_score(True)
    numeric = numeric_equivalent(predicted, target)
    if numeric is not None:
        return binary_score(numeric)
    normalize = lambda text: re.sub(r"[^A-Za-z0-9.\-/]", "", text).lower()
    left, right = normalize(predicted), normalize(target)
    return binary_score(bool(left and right and left == right))



def score_olymmath(answer, private_data):
    target = private_data["answer"].strip()
    boxed = extract_boxed(answer)
    predicted = (boxed if boxed is not None else answer.rsplit("####", 1)[-1]).strip()
    if not target or not predicted:
        return binary_score(False)
    if predicted == target:
        return binary_score(True)
    predicted = _assignment(predicted)
    numeric = numeric_equivalent(predicted, target)
    if numeric is not None:
        return binary_score(numeric)
    # Explicit reference-policy correction to the inspected Rust fallback.
    supported = {"frac", "dfrac", "tfrac", "cdot", "times", "left", "right"}
    if any(command not in supported for command in re.findall(r"\\([A-Za-z]+)", predicted + " " + target)):
        return binary_score(False)
    left, right = _assignment(_math_normalize(predicted)), _math_normalize(target)
    return binary_score(bool(left and right and left == right))



def _label(text, aliases, phrases, words):
    lower = text.strip().translate(str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"))
    if lower in aliases:
        return aliases[lower]
    best = (-1, None)
    for canonical, phrase in phrases:
        position = lower.rfind(phrase)
        if position >= 0 and position >= best[0]:
            best = (position, canonical)
    for canonical, word in words:
        matches = list(re.finditer(r"(?<![A-Za-z0-9])" + re.escape(word) + r"(?![A-Za-z0-9])", lower))
        if matches and matches[-1].start() >= best[0]:
            best = (matches[-1].start(), canonical)
    return best[1]


PUBMEDQA_ALIASES = {"yes":"yes", "y":"yes", "no":"no", "n":"no", "maybe":"maybe", "uncertain":"maybe"}
PUBMEDQA_PHRASES = ()
PUBMEDQA_WORDS = (("yes","yes"),("no","no"),("maybe","maybe"))


def score_pubmedqa(answer, private_data):
    return score_label(answer, private_data, PUBMEDQA_ALIASES, PUBMEDQA_PHRASES, PUBMEDQA_WORDS)



SCITAB_ALIASES = {**dict.fromkeys(("supports","support","supported","true"),"supports"),
           **dict.fromkeys(("refutes","refute","refuted","false"),"refutes"),
           **dict.fromkeys(("not enough info","not enough information","nei","insufficient","insufficient information","unverifiable"),"not enough info")}
SCITAB_PHRASES = tuple(("not enough info", value) for value in ("not enough info","not enough information","insufficient information","insufficient evidence"))
SCITAB_WORDS = tuple((key,value) for key, values in (("supports",("supports","support","supported")),("refutes",("refutes","refute","refuted")),("not enough info",("nei",))) for value in values)


def score_scitab(answer, private_data):
    return score_label(answer, private_data, SCITAB_ALIASES, SCITAB_PHRASES, SCITAB_WORDS)



def score_label(answer, private_data, aliases, phrases, words):
    expected = _label(private_data["answer"], aliases, phrases, words)
    predicted = _label(answer, aliases, phrases, words)
    return binary_score(expected is not None and predicted == expected)

