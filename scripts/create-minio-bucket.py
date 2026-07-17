#!/usr/bin/env python3
"""用 SigV4 幂等创建 MinIO bucket（仅标准库，无 boto3）。"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import hmac
import sys
import urllib.error
import urllib.request


def _sign(key: bytes, msg: str) -> bytes:
    return hmac.new(key, msg.encode("utf-8"), hashlib.sha256).digest()


def _sigv4_key(secret: str, datestamp: str, region: str, service: str) -> bytes:
    k_date = _sign(("AWS4" + secret).encode("utf-8"), datestamp)
    k_region = _sign(k_date, region)
    k_service = _sign(k_region, service)
    return _sign(k_service, "aws4_request")


def create_bucket(endpoint: str, access: str, secret: str, bucket: str, region: str = "us-east-1") -> str:
    endpoint = endpoint.rstrip("/")
    host = endpoint.split("://", 1)[1]
    now = datetime.datetime.now(datetime.timezone.utc)
    amz_date = now.strftime("%Y%m%dT%H%M%SZ")
    datestamp = now.strftime("%Y%m%d")
    service = "s3"
    payload_hash = hashlib.sha256(b"").hexdigest()
    canonical_uri = f"/{bucket}"
    canonical_headers = (
        f"host:{host}\n"
        f"x-amz-content-sha256:{payload_hash}\n"
        f"x-amz-date:{amz_date}\n"
    )
    signed_headers = "host;x-amz-content-sha256;x-amz-date"
    canonical_request = "\n".join(
        ["PUT", canonical_uri, "", canonical_headers, signed_headers, payload_hash]
    )
    credential_scope = f"{datestamp}/{region}/{service}/aws4_request"
    string_to_sign = "\n".join(
        [
            "AWS4-HMAC-SHA256",
            amz_date,
            credential_scope,
            hashlib.sha256(canonical_request.encode("utf-8")).hexdigest(),
        ]
    )
    signature = hmac.new(
        _sigv4_key(secret, datestamp, region, service),
        string_to_sign.encode("utf-8"),
        hashlib.sha256,
    ).hexdigest()
    authorization = (
        f"AWS4-HMAC-SHA256 Credential={access}/{credential_scope}, "
        f"SignedHeaders={signed_headers}, Signature={signature}"
    )
    req = urllib.request.Request(
        f"{endpoint}/{bucket}",
        method="PUT",
        headers={
            "Host": host,
            "x-amz-content-sha256": payload_hash,
            "x-amz-date": amz_date,
            "Authorization": authorization,
        },
    )
    try:
        with urllib.request.urlopen(req) as resp:
            return f"created:{resp.status}"
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        if exc.code in (200, 409) or "BucketAlreadyOwnedByYou" in body or "BucketAlreadyExists" in body:
            return f"exists:{exc.code}"
        raise SystemExit(f"CreateBucket failed: {exc.code} {body}") from exc


def main() -> int:
    parser = argparse.ArgumentParser(description="幂等创建 MinIO bucket（SigV4）")
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--access-key", required=True)
    parser.add_argument("--secret-key", required=True)
    parser.add_argument("--bucket", required=True)
    args = parser.parse_args()
    print(
        create_bucket(
            endpoint=args.endpoint,
            access=args.access_key,
            secret=args.secret_key,
            bucket=args.bucket,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
