#!/usr/bin/env python3
import json, pathlib, sys
root=pathlib.Path(__file__).resolve().parents[1]; source=json.loads((root/'contracts/integration-profile.schema.json').read_text()); generated=json.loads((root/'docs/contracts/generated/integration-profile.schema.json').read_text()); base=json.loads((root/'contracts/base-vocabulary.schema.json').read_text()); base_generated=json.loads((root/'docs/contracts/generated/base-vocabulary.schema.json').read_text())
if source != generated: sys.exit('Cortex integration contract drift')
if base != base_generated: sys.exit('Cortex base vocabulary drift')
def ok(v): return v.get('product')=='cortex' and v.get('contract_version')=='1.0.0' and v.get('api_version',{}).get('major')==1 and str(v.get('server_id','')).startswith('cortex_')
fixtures=root/'contracts/fixtures/integration'; valid=json.loads((fixtures/'valid.json').read_text()); wrong=json.loads((fixtures/'wrong-product.json').read_text()); major=json.loads((fixtures/'unsupported-major.json').read_text())
if not ok(valid) or ok(wrong) or ok(major): sys.exit('Cortex compatibility fixtures failed')
redacted=(fixtures/'redacted-error.json').read_text().lower()
if any(secret in redacted for secret in ('bearer ','api_key','access_token','client_secret','password')): sys.exit('Cortex redacted error fixture contains credential material')
print('Cortex integration contract: schema drift, 3 compatibility fixtures, and redaction fixture passed')
