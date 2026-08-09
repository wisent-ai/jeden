#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
from pathlib import Path


def canonical(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode("utf-8")


def write_json(path: Path, value: object) -> bytes:
    encoded = canonical(value) + b"\n"
    path.write_bytes(encoded)
    return encoded


parser = argparse.ArgumentParser()
parser.add_argument("--binary", required=True)
parser.add_argument("--receipts", required=True)
parser.add_argument("--version", required=True)
parser.add_argument("--platform", required=True)
args = parser.parse_args()

binary = Path(args.binary)
receipts = Path(args.receipts)
receipts.mkdir(parents=True, exist_ok=True)
binary_bytes = binary.read_bytes()
binary_sha256 = hashlib.sha256(binary_bytes).hexdigest()
binary_sha1 = hashlib.sha1(binary_bytes, usedforsecurity=False).hexdigest()
package_verification_code = hashlib.sha1(binary_sha1.encode("ascii"), usedforsecurity=False).hexdigest()
subject = {"name": "jeden", "digest": {"sha256": binary_sha256}}

sbom = {
    "SPDXID": "SPDXRef-DOCUMENT",
    "creationInfo": {
        "created": "1970-01-01T00:00:00Z",
        "creators": ["Tool: jeden-release-evidence-v1"],
    },
    "dataLicense": "CC0-1.0",
    "documentNamespace": f"stado://receipts/jeden/{args.version}/{args.platform}/sbom/{binary_sha256}",
    "files": [{
        "SPDXID": "SPDXRef-File-jeden",
        "checksums": [
            {"algorithm": "SHA1", "checksumValue": binary_sha1},
            {"algorithm": "SHA256", "checksumValue": binary_sha256},
        ],
        "fileName": "jeden",
    }],
    "name": f"jeden-{args.version}-{args.platform}",
    "packages": [{
        "SPDXID": "SPDXRef-Package-jeden",
        "downloadLocation": "NOASSERTION",
        "filesAnalyzed": True,
        "name": "jeden",
        "packageVerificationCode": {"packageVerificationCodeValue": package_verification_code},
        "versionInfo": args.version,
    }],
    "relationships": [
        {"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES", "relatedSpdxElement": "SPDXRef-Package-jeden"},
        {"spdxElementId": "SPDXRef-Package-jeden", "relationshipType": "CONTAINS", "relatedSpdxElement": "SPDXRef-File-jeden"},
    ],
    "spdxVersion": "SPDX-2.3",
}
sbom_bytes = write_json(receipts / "sbom.spdx.json", sbom)

provenance = {
    "_type": "https://in-toto.io/Statement/v1",
    "predicateType": "https://slsa.dev/provenance/v1",
    "subject": [subject],
    "predicate": {
        "buildDefinition": {
            "buildType": "https://stado.wisent.com/build-types/wisent-release/v1",
            "externalParameters": {"platform": args.platform, "version": args.version},
            "internalParameters": {"lockedDependencies": True},
            "resolvedDependencies": [{"uri": f"stado://sources/jeden/{args.version}"}],
        },
        "runDetails": {"builder": {"id": "stado://release-pipeline/v1"}},
    },
}
provenance_bytes = write_json(receipts / "provenance.intoto.json", provenance)

evidence_statement = {
    "_type": "https://in-toto.io/Statement/v1",
    "predicateType": "https://stado.wisent.com/predicates/release-evidence/v1",
    "subject": [subject],
    "predicate": {
        "platform": args.platform,
        "version": args.version,
        "receipts": {
            "provenanceSha256": hashlib.sha256(provenance_bytes).hexdigest(),
            "sbomSha256": hashlib.sha256(sbom_bytes).hexdigest(),
        },
        "signingAuthority": "stado-skarbiec",
    },
}
payload = canonical(evidence_statement)
dsse_evidence = {
    "payload": base64.b64encode(payload).decode("ascii"),
    "payloadType": "application/vnd.in-toto+json",
    "signatures": [],
    "stadoReceiptRequired": True,
}
write_json(receipts / "dsse-evidence.json", dsse_evidence)
