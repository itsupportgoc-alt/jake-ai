from jarvis_py.redact import looks_like_secret, redact, redact_obj


def test_masks_api_keys():
    out = redact("using key sk-abcdef1234567890abcdef1234567890 now")
    assert "sk-abcdef" not in out
    assert "redacted" in out


def test_masks_assignments():
    for text in [
        "api_key=sk-123456789012345678901234",
        "api_key: hunter2",
        "password = hunter2",
        "TOKEN=abcdef1234567890",
        "Authorization: Bearer abcdef1234567890",
    ]:
        out = redact(text)
        assert "hunter2" not in out, out
        assert "abcdef1234567890" not in out, out
        assert "sk-123456789012345678901234" not in out, out


def test_masks_github_jwt_google():
    assert "ghp_" not in redact("token ghp_abcdefghijklmnopqrstuvwx1234 leaked")
    assert "eyJhbGci" not in redact("auth eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abc")
    assert "AIzaSy" not in redact("key AIzaSyAbCdEf1234567890")


def test_leaves_ordinary_text_alone():
    text = "Opened Chrome and read C:/Users/USER/report.txt at 12:04 (desk-1234)"
    assert redact(text) == text
    assert redact("the author wrote this") == "the author wrote this"


def test_word_boundary_for_prefixes():
    # "desk-1234" must not be treated as sk-...
    assert redact("desk-1234") == "desk-1234"


def test_redact_obj_recurses():
    obj = {"a": "sk-abcdefghijklmnop123456", "b": ["ok", {"password": "hunter2"}], "c": 3}
    out = redact_obj(obj)
    assert "abcdefghijklmnop123456" not in out["a"]
    assert "hunter2" not in out["b"][1]["password"]
    assert out["c"] == 3


def test_looks_like_secret():
    assert looks_like_secret("my api_key is sk-abcdef1234567890abcdef1234567890")
    assert not looks_like_secret("open the inventory project")