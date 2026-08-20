"""Self-tests for harness script machinery that CI's plugin pipeline does
not exercise: the archive-member fixture paths (extraction, flattening,
rejection) and load_harness_toml's members validation."""

import io
import json
import sys
import tempfile
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import harness_build
import rvcommon


def expect_fail(fn, label):
    try:
        fn()
    except SystemExit:
        print(f"ok: {label} rejected")
        return
    raise AssertionError(f"{label}: expected HARNESS CHECK FAILED, got success")


def sha256(data):
    import hashlib

    return hashlib.sha256(data).hexdigest()


def make_zip(entries):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        for name, data in entries.items():
            zf.writestr(name, data)
    return buf.getvalue()


def fixture_cfg(file, digest, members=None):
    f = {"file": file, "sha256": digest}
    if members is not None:
        f["members"] = members
    return {"fixtures": [f]}


def test_fixture_files(tmp):
    repo = tmp / "repo"
    (repo / "fixtures").mkdir(parents=True)
    work = tmp / "work"

    plain = b"plain fixture bytes"
    (repo / "fixtures" / "song.mod").write_bytes(plain)
    out = harness_build.fixture_files(
        repo, fixture_cfg("fixtures/song.mod", sha256(plain)), work
    )
    assert out == [repo / "fixtures" / "song.mod"], out
    print("ok: plain fixture passes through")

    zdata = make_zip({"song.mdx": b"mdx bytes", "sub/dir/nested.mdx": b"nested bytes"})
    (repo / "fixtures" / "songs.zip").write_bytes(zdata)
    zsha = sha256(zdata)

    out = harness_build.fixture_files(
        repo, fixture_cfg("fixtures/songs.zip", zsha, ["song.mdx"]), work
    )
    assert len(out) == 1 and out[0].read_bytes() == b"mdx bytes", out
    print("ok: member extracted")

    out = harness_build.fixture_files(
        repo, fixture_cfg("fixtures/songs.zip", zsha, ["sub/dir/nested.mdx"]), work
    )
    assert out[0].name == "nested.mdx" and out[0].parent == work / "fixtures-extracted"
    assert out[0].read_bytes() == b"nested bytes"
    print("ok: nested member flattened to basename inside the work dir")

    expect_fail(
        lambda: harness_build.fixture_files(
            repo, fixture_cfg("fixtures/songs.zip", zsha, ["absent.mdx"]), work
        ),
        "missing member",
    )

    notzip = b"not a zip at all"
    (repo / "fixtures" / "notzip.zip").write_bytes(notzip)
    expect_fail(
        lambda: harness_build.fixture_files(
            repo, fixture_cfg("fixtures/notzip.zip", sha256(notzip), ["x"]), work
        ),
        "members on a non-zip",
    )

    cdata = make_zip({"song.mdx": b"a", "sub/song.mdx": b"b"})
    (repo / "fixtures" / "collide.zip").write_bytes(cdata)
    expect_fail(
        lambda: harness_build.fixture_files(
            repo,
            fixture_cfg("fixtures/collide.zip", sha256(cdata), ["song.mdx", "sub/song.mdx"]),
            work,
        ),
        "basename collision after extraction",
    )


def test_members_validation(tmp):
    good = b"data"
    for members, label in (
        ("notalist", "members not a list"),
        ([7], "non-string member"),
        ([""], "empty member"),
        (["/abs"], "absolute member"),
        (["\\abs"], "backslash-absolute member"),
        (["a/../b"], "traversal member"),
    ):
        repo = tmp / f"toml-{label.replace(' ', '-')}"
        (repo / "fixtures").mkdir(parents=True)
        (repo / "fixtures" / "f.zip").write_bytes(good)
        # json.dumps emits double-quoted strings, which TOML parses as basic
        # strings with escape processing — so the parsed value round-trips.
        members_toml = json.dumps(members)
        (repo / "harness.toml").write_text(
            "[plugin]\nname = \"t\"\n"
            "[[fixtures]]\nfile = \"fixtures/f.zip\"\n"
            f"sha256 = \"{sha256(good)}\"\nmembers = {members_toml}\n"
        )
        expect_fail(lambda r=repo: rvcommon.load_harness_toml(r), label)

    repo = tmp / "toml-good"
    (repo / "fixtures").mkdir(parents=True)
    (repo / "fixtures" / "f.zip").write_bytes(good)
    (repo / "harness.toml").write_text(
        "[plugin]\nname = \"t\"\n"
        "[[fixtures]]\nfile = \"fixtures/f.zip\"\n"
        f"sha256 = \"{sha256(good)}\"\nmembers = [\"a.mdx\", \"sub/b.mdx\"]\n"
    )
    cfg = rvcommon.load_harness_toml(repo)
    assert cfg["fixtures"][0]["members"] == ["a.mdx", "sub/b.mdx"]
    print("ok: valid members accepted")


def main():
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        test_fixture_files(tmp / "ff")
        test_members_validation(tmp / "toml")
    print("selftest: all checks passed")


if __name__ == "__main__":
    main()
