#!/usr/bin/env python3
"""Update deploy-manifest.json with core contract addresses."""
import json, os

MANIFEST = os.path.join(os.path.dirname(__file__), '..', 'deploy-manifest.json')

manifest = json.load(open(MANIFEST))

core_contracts = {
    'lichencoin': 'EesNHAy58XtP6E7U5QiRJwmyuPY1XqCx434cMBHXTue7',
    'lichendao': 'Eei6jKysR31E4ryKMtmQQ5ac5F974kkQM9tgejtLMnE7',
    'lichenswap': 'Cf1MLrmNCmuEGuK4nhsUUJwZT4vzftLNPHHX3tJWYsG9',
    'lichenbridge': 'ZqAhYhNaa9iZt6jW6Vcbu84rBGRgUjGz3PFHVeHGQsZ',
    'lichenmarket': '5w2jprhbrHn74FJTgpnDZtWQ8k9b9vNEHuzA6cRDsmz2',
    'lichenoracle': '3Xd2cTZiwCQhEpQMp5PTzo7uJu5f1KiaPV82YooreCMw',
    'lichenauction': 'AUp8bQgRkAJvVSHuqqfZbX5zCJMSEB3gmqeDkSFBkBqb',
    'lichenpunks': 'BAFPvUgUxa9ZFYi49WyJckejaZey7iEEj4Ca5GdY9PJ6',
    'lichenid': '2EkxTqat1vur3PGFo935pDkAP7uTvYYPF2C19AyHTo9M',
    'thalllend': 'CFbauYWDfqMYGQf4nDF8n2D8ECdMYbjbjsMUCwpTWDvw',
    'sporepay': '8VLhXaHRrYKXomqdGSyDVkVF5GCqQZxUJuEbQsdSD1YC',
    'sporepump': '71mCQZTFAbnm1cYGCM364A95ARzwrTBKxTzP892u2zSh',
    'sporevault': '9wqC3cwb1Q9pqmFVUzA5GLHPmKtjfUc9KsASuff8Q3Kd',
    'bountyboard': 'DtzzWJ2dLkvujcQUvAgaWaVVcg6g169CTaSN6Ye5RXT7',
    'compute_market': '5WzGdwDjnPkirQ3yLG8fMFT2t2JXYzLbCmTWnvhs5ZxM',
    'moss_storage': 'AYYj9Xkp7B49FcLebHzNYu2JY5nvqE6koY4t749JATK3',
    'shielded_pool': 'HG7Cc8AiYujYBUqvVUkzZJwUdYSUYL76n7DjzKcnHMra',
}

manifest['contracts'].update(core_contracts)
manifest['core_contracts'] = core_contracts

with open(MANIFEST, 'w') as f:
    json.dump(manifest, f, indent=2)

print(f"Manifest updated: {len(manifest['contracts'])} contracts total")
for name, addr in sorted(manifest['contracts'].items()):
    print(f"  {name:20s} -> {addr}")
