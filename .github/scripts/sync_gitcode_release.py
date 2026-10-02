import json
import os
import pathlib
import hashlib
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from urllib.parse import urlparse


API = "https://api.gitcode.com/api/v5/repos"


def request(url, method="GET", payload=None, headers=None, allow_404=False):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=25) as response:
            body = response.read()
            return response.status, json.loads(body) if body else {}
    except urllib.error.HTTPError as error:
        # Never print request URLs: GitCode's documented token parameter is in query.
        if allow_404 and error.code == 404:
            return 404, {}
        raise RuntimeError(f"GitCode API returned HTTP {error.code}") from None
    except (urllib.error.URLError, TimeoutError) as error:
        raise RuntimeError(f"GitCode API connection failed ({type(error).__name__})") from None


def main():
    repo = os.environ.get("GITCODE_REPOSITORY", "").strip("/")
    token = os.environ.get("GITCODE_TOKEN", "")
    tag = os.environ.get("RELEASE_TAG", "")
    if not repo:
        raise RuntimeError("GITCODE_REPOSITORY variable is missing")
    if not tag:
        raise RuntimeError("RELEASE_TAG is missing")
    if not re.fullmatch(r"[A-Za-z0-9_-]+/[A-Za-z0-9_.-]+", repo):
        raise RuntimeError("GITCODE_REPOSITORY must be owner/repository")
    if not token:
        raise RuntimeError("GITCODE_TOKEN secret is missing")
    base = f"{API}/{repo}/releases"
    auth = urllib.parse.urlencode({"access_token": token})
    tag_path = urllib.parse.quote(tag, safe="")
    status, release = request(f"{base}/tags/{tag_path}?{auth}", allow_404=True)
    if status == 404:
        _, release = request(f"{base}?{auth}", "POST", {
            "tag_name": tag,
            "name": os.environ.get("RELEASE_NAME") or tag,
            "body": os.environ.get("RELEASE_BODY", ""),
            "target_commitish": os.environ["RELEASE_TARGET"],
            "release_status": "latest",
        }, {"Content-Type": "application/json", "Accept": "application/json"})
    else:
        # Make re-runs converge on GitHub's release notes.
        release_id = urllib.parse.quote(str(release["id"]), safe="")
        request(f"{base}/{release_id}?{auth}", "PATCH", {
            "tag_name": tag,
            "name": os.environ.get("RELEASE_NAME") or tag,
            "body": os.environ.get("RELEASE_BODY", ""),
        }, {"Content-Type": "application/json", "Accept": "application/json"})

    existing = {item.get("name"): item.get("browser_download_url") for item in release.get("assets", [])}
    for path in sorted(pathlib.Path(os.environ["RUNNER_TEMP"], "release").iterdir()):
        if not path.is_file():
            continue
        if path.name in existing:
            url = existing[path.name]
            parsed = urlparse(url or "")
            if parsed.scheme != "https" or parsed.hostname != "gitcode.com":
                raise RuntimeError(f"Existing GitCode asset URL is invalid for {path.name}")
            try:
                with urllib.request.urlopen(url, timeout=60) as response:
                    mirror_bytes = response.read(256 * 1024 * 1024 + 1)
            except (urllib.error.URLError, TimeoutError):
                raise RuntimeError(f"Could not verify existing GitCode asset {path.name}") from None
            if len(mirror_bytes) > 256 * 1024 * 1024 or hashlib.sha256(mirror_bytes).digest() != hashlib.sha256(path.read_bytes()).digest():
                raise RuntimeError(f"Existing GitCode asset differs from GitHub asset: {path.name}; investigate before retrying")
            print(f"Already mirrored and verified: {path.name}")
            continue
        query = urllib.parse.urlencode({"access_token": token, "file_name": path.name})
        _, upload = request(f"{base}/{tag_path}/upload_url?{query}", headers={"Accept": "application/json"})
        upload_url = upload["url"]
        headers = upload.get("headers", {})
        data = path.read_bytes()
        req = urllib.request.Request(upload_url, data=data, method="PUT", headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=180) as response:
                if not 200 <= response.status < 300:
                    raise RuntimeError(f"GitCode asset upload failed for {path.name}: HTTP {response.status}")
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"GitCode asset upload failed for {path.name}: HTTP {error.code}") from None
        except (urllib.error.URLError, TimeoutError) as error:
            raise RuntimeError(f"GitCode asset upload failed for {path.name} ({type(error).__name__})") from None
        print(f"Mirrored asset: {path.name}")
    print(f"GitHub release {tag} mirrored to GitCode {repo}")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"GitCode release mirror failed: {error}", file=sys.stderr)
        sys.exit(1)
