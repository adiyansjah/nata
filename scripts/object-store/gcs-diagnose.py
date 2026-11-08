#!/usr/bin/env python3
"""Stream/discard object bodies. Emit header names and allowlisted values only."""
import http.client
import json
import os
from pathlib import Path
import runpy
import sys
import time
from urllib.parse import quote

apply = runpy.run_path(str(Path(__file__).with_name('redact.py')))['redactor'](os.environ)
SAFE = {'content-length', 'content-range', 'content-encoding',
        'x-goog-stored-content-encoding', 'x-goog-stored-content-length', 'accept-ranges'}
OBJECTS = ['sf1/mid1/part-0000.parquet', 'check/sqlcheck/types.parquet',
           'check/lineitem.parquet', 'sf1/files100/part-0000.parquet',
           'sf1/small1/part-0000.parquet']


def emit(**values):
    sys.stdout.buffer.write(apply((json.dumps(values, sort_keys=True) + '\n').encode()))
    sys.stdout.buffer.flush()


def probe(host, path, method, headers, limit=2 << 30, tls=True):
    conn = (http.client.HTTPSConnection if tls else http.client.HTTPConnection)(host, timeout=30)
    start = time.monotonic()
    try:
        conn.request(method, path, headers=headers)
        response = conn.getresponse()
        pairs = response.getheaders()
        selected = {name.lower(): value for name, value in pairs if name.lower() in SAFE}
        names = sorted({name.lower() for name, _ in pairs})
        count = 0
        complete = False
        while count <= limit and time.monotonic() - start < 180:
            data = response.read(min(1 << 20, limit + 1 - count))
            count += len(data)
            if not data:
                complete = True
                break
        # Return validators for comparison in memory, never print their values.
        validators = {name.lower(): value for name, value in pairs
                      if name.lower() in ('etag', 'x-goog-generation')}
        return dict(status=response.status, header_names=names, safe_headers=selected,
                    body_bytes=count, complete=complete), validators
    finally:
        conn.close()


def main():
    # Fixed link-local metadata host; redirects are not followed. Token stays in RAM.
    conn = http.client.HTTPConnection('169.254.169.254', timeout=5)
    try:
        conn.request('GET', '/computeMetadata/v1/instance/service-accounts/default/token',
                     headers={'Metadata-Flavor': 'Google'})
        response = conn.getresponse()
        doc = response.read((8 << 20) + 1)
        emit(probe='metadata-token', status=response.status,
             header_names=sorted({k.lower() for k, _ in response.getheaders()}), body_bytes=len(doc))
        if response.status != 200 or len(doc) > 8 << 20:
            return 1
        token = json.loads(doc)['access_token']
    finally:
        conn.close()
    headers = {'Authorization': 'Bearer ' + token}
    bucket, prefix = os.environ['NATA_OBJ_BUCKET'], os.environ['NATA_OBJ_PREFIX'].strip('/')
    failed = False
    for relative in sys.argv[1:] or OBJECTS:
        key = prefix + '/' + relative
        path = '/' + quote(bucket, safe='') + '/' + quote(key, safe='/')
        alias = 'gs://<bucket>/<prefix>/' + relative
        head_validators = {}
        size = None
        requests = [('HEAD', None), ('GET', None), ('GET', 'bytes=0-7'), ('GET', 'bytes=-8')]
        # Append full-object and past-EOF ranges after HEAD establishes the length.
        for method, requested in requests:
            label = method if requested is None else requested
            try:
                result, validators = probe('storage.googleapis.com', path, method,
                                           dict(headers, **({'Range': requested} if requested else {})))
                if method == 'HEAD':
                    head_validators = validators
                    length = result['safe_headers'].get('content-length', '')
                    if result['status'] == 200 and length.isdecimal():
                        size = int(length)
                        if size:
                            requests.extend([('GET', f'bytes=0-{size - 1}'),
                                             ('GET', f'bytes={size}-{size + 7}')])
                result['validators_equal_head'] = {
                    name: validators.get(name) == value for name, value in head_validators.items()}
                emit(object=alias, probe=label, **result)
                if not result['complete']:
                    failed = True
            except Exception as error:
                # Exception text can include URLs/headers; keep its class only.
                emit(object=alias, probe=label, error_class=type(error).__name__)
                failed = True
        # JSON metadata has its own ETag semantics; do not use it to pin XML reads.
        conn = http.client.HTTPSConnection('storage.googleapis.com', timeout=30)
        try:
            json_path = '/storage/v1/b/' + quote(bucket, safe='') + '/o/' + quote(key, safe='')
            conn.request('GET', json_path + '?fields=size,contentEncoding,contentType,etag,generation', headers=headers)
            response = conn.getresponse()
            doc = response.read((8 << 20) + 1)
            record = dict(object=alias, probe='JSON-metadata', status=response.status,
                          header_names=sorted({k.lower() for k, _ in response.getheaders()}), body_bytes=len(doc))
            if response.status == 200 and len(doc) <= 8 << 20:
                metadata = json.loads(doc)
                record.update(metadata_names=sorted(metadata),
                              size=metadata.get('size'), content_encoding=metadata.get('contentEncoding'),
                              content_type=metadata.get('contentType'),
                              size_equal_xml_head=str(size) == metadata.get('size'),
                              generation_equal_xml_head=metadata.get('generation') == head_validators.get('x-goog-generation'),
                              etag_equal_xml_head=metadata.get('etag') == head_validators.get('etag'))
            emit(**record)
        except Exception as error:
            emit(object=alias, probe='JSON-metadata', error_class=type(error).__name__)
            failed = True
        finally:
            conn.close()
        # Local size helps distinguish upload metadata from changed object bytes.
        work = Path(os.environ.get('WORK', '/nonexistent'))
        local = work / ('data/tpch-sf0.01/' + relative[6:] if relative.startswith('check/')
                        else 'osb-data/lay/' + relative)
        if local.is_file():
            emit(object=alias, probe='local-file', body_bytes=local.stat().st_size)
    return int(failed)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception as error:
        emit(probe='diagnostic', error_class=type(error).__name__)
        sys.exit(1)
