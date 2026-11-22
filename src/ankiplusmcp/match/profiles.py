"""Find the student's Anki profiles, so a deck can be indexed without exporting it.

Anki keeps one folder per profile under its base folder, each with ``collection.anki2``:

- macOS: ``~/Library/Application Support/Anki2``
- Windows: ``%APPDATA%\\Anki2``
- Linux: ``$XDG_DATA_HOME/Anki2`` (default ``~/.local/share/Anki2``)

``ANKI_BASE`` overrides it (Anki's own ``-b`` folder). The collection is only ever read
from a private copy (see ``deck.load_deck``); Anki can stay open.
"""
import os
from pathlib import Path
import sys

SKIP = {'addons21', 'logs', 'crash.log'}


def anki_base(env=None, platform=None, home=None):
    env = os.environ if env is None else env
    platform = platform or sys.platform
    home = Path(home or Path.home())
    if env.get('ANKI_BASE'):
        return Path(env['ANKI_BASE']).expanduser()
    if platform == 'darwin':
        return home / 'Library' / 'Application Support' / 'Anki2'
    if platform.startswith('win'):
        return Path(env.get('APPDATA') or home / 'AppData' / 'Roaming') / 'Anki2'
    return Path(env.get('XDG_DATA_HOME') or home / '.local' / 'share') / 'Anki2'


def list_profiles(base=None):
    base = Path(base) if base else anki_base()
    out = []
    if base.is_dir():
        for folder in sorted(base.iterdir()):
            col = folder / 'collection.anki2'
            if folder.name not in SKIP and col.is_file():
                stat = col.stat()
                out.append({'profile': folder.name, 'collection': str(col), 'bytes': stat.st_size, 'modified': int(stat.st_mtime)})
    return out


def collection_for(profile=None, base=None):
    """The collection file for ``profile``; with no name, the only profile (or 'User 1')."""
    found = list_profiles(base)
    if not found:
        raise FileNotFoundError(f'No Anki profiles found in {Path(base) if base else anki_base()}. '
                                'Open Anki once, or set ANKI_BASE to its data folder.')
    if profile:
        match = [p for p in found if p['profile'] == profile]
        if not match:
            raise FileNotFoundError(f"No Anki profile named {profile!r}. Profiles: {', '.join(p['profile'] for p in found)}.")
        return Path(match[0]['collection'])
    if len(found) == 1:
        return Path(found[0]['collection'])
    default = [p for p in found if p['profile'] == 'User 1']
    if default:
        return Path(default[0]['collection'])
    raise FileNotFoundError(f"Several Anki profiles: {', '.join(p['profile'] for p in found)}. Name one.")
