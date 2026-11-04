"""Tag adapter backed by the Rust PuddleMore engine.

The puddletag UI can read and write formats that mutagen does not cover
(Matroska, APEv2-only TrueAudio and native AIFF chunks) by delegating to the
Rust binary. The adapter keeps the normal puddletag Tag API, so the rest of
the UI is unchanged.
"""
import json
import os
import shutil
import subprocess

from .util import (MockTag, CaselessDict, fn_hash, isempty, usertags,
                   cover_info, getinfo, get_filename_tags)

FORMATS = {
    'mkv': 'Matroska Video',
    'mka': 'Matroska Audio',
    'webm': 'WebM',
    'tta': 'TrueAudio',
    'aif': 'AIFF',
    'aiff': 'AIFF',
    'aifc': 'AIFF',
    'wav': 'WAV',
    'wave': 'WAV',
}

BINARY_CANDIDATES = (
    os.path.join(os.path.dirname(__file__), '..', '..', 'rust', 'target',
                 'release', 'puddlemore'),
    os.path.join(os.path.dirname(__file__), '..', '..', 'rust', 'target',
                 'debug', 'puddlemore'),
)


def binary():
    override = os.environ.get('PUDDLEMORE_BIN')
    if override and os.path.isfile(override):
        return override
    for candidate in BINARY_CANDIDATES:
        candidate = os.path.abspath(candidate)
        if os.path.isfile(candidate):
            return candidate
    return shutil.which('puddlemore')


def _read(path):
    exe = binary()
    if not exe:
        raise IOError('PuddleMore Rust engine not found')
    output = subprocess.check_output(
        [exe, '--print', '--json', path], text=True, errors='replace')
    rows = json.loads(output)
    if not rows:
        raise IOError('no metadata returned')
    return rows[0].get('tags', {})


def _split(value):
    if isinstance(value, list):
        return [str(v) for v in value]
    return str(value).split('\\\\')


class RustTag(MockTag):
    mapping = {}
    revmapping = {}
    IMAGETAGS = ()

    def __init__(self, filename=None):
        self.__tags = CaselessDict()
        self.__images = []
        self.__original = {}
        ext = os.path.splitext(str(filename))[1][1:].lower() if filename else ''
        self.filetype = FORMATS.get(ext, 'Audio')
        MockTag.__init__(self, filename)

    @property
    def images(self):
        return self.__images

    @images.setter
    def images(self, images):
        self.__images = images or []
        cover_info(images, self.__tags)

    def __contains__(self, key):
        if key == '__image':
            return bool(self.__images)
        if key.startswith('__'):
            return key in self.__tags
        return key in self.__tags

    def __getitem__(self, key):
        if key == '__image':
            return self.__images
        value = self.__tags[key]
        if key.startswith('__') and isinstance(value, list):
            return value[0] if value else ''
        return value

    def __setitem__(self, key, value):
        if key == '__image':
            self.images = value
            return
        if key.startswith('__'):
            if key in fn_hash:
                setattr(self, fn_hash[key], value)
            return
        if isempty(value):
            self.__tags.pop(key, None)
            return
        if isinstance(value, str):
            value = [value]
        self.__tags[key] = [str(v) for v in value]

    def __delitem__(self, key):
        if key == '__image':
            self.images = []
        else:
            self.__tags.pop(key, None)

    def keys(self):
        return list(self.__tags.keys())

    def link(self, filename):
        self.filepath = filename
        fields = _read(filename)
        self.__tags = CaselessDict()
        self.__tags.update(get_filename_tags(self.filepath))
        self.__tags.update(getinfo(filename))
        for key, value in fields.items():
            if key == '__image':
                continue
            values = _split(value)
            if values:
                self.__tags[key] = values
        self.__original = {k: list(v) for k, v in usertags(self.__tags).items()}
        self.__tags['__tag_read'] = self.filetype
        self.set_attrs(('frequency', 'length', 'bitrate', 'size', 'channels',
                        'bitspersample'))
        self.update_tag_list()
        return self

    def save(self):
        exe = binary()
        if not exe:
            raise IOError('PuddleMore Rust engine not found')
        fields = usertags(self.__tags)
        args = [exe]
        for key, values in fields.items():
            joined = '\\\\'.join(str(v) for v in values)
            args.extend(['--set', '%s=%s' % (key, joined)])
        for key in self.__original:
            if key not in fields:
                args.extend(['--set', '%s=' % key])
        args.append(self.filepath)
        subprocess.run(args, check=True, stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE)
        self.link(self.filepath)
        return True

    def update_tag_list(self):
        self.__tags['__tag'] = self.filetype

    @property
    def info(self):
        return [('File', [('Path', self.filepath),
                          ('Size', self.__tags.get('__file_size', [''])[0]),
                          ('Filename', self.filename),
                          ('Modified', self.__tags.get('__modified', [''])[0])]),
                (self.filetype, [('Codec', self.__tags.get('__codec', ['-'])[0]),
                                 ('Frequency', self.__tags.get('__frequency', ['-'])[0]),
                                 ('Channels', self.__tags.get('__channels', ['-'])[0]),
                                 ('Length', self.__tags.get('__length', ['-'])[0])])]


def register():
    from . import extensions, options
    for ext, label in FORMATS.items():
        entry = [RustTag, RustTag, label]
        extensions[ext] = entry
        if entry not in options:
            options.append(entry)
