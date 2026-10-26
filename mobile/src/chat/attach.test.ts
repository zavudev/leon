import { describe, expect, test } from 'bun:test';

import { bytesOfBase64, fileLabel, mimeOfDataUrl, nameOf, splitAttached, weight, withAttached } from './attach';

describe('withAttached', () => {
  test('the words, then each path on a line, as the desktop writes them', () => {
    expect(withAttached('Look at this', ['/tmp/a.png', '/tmp/b.pdf'])).toBe(
      'Look at this\n\nAttached (open each path):\n- /tmp/a.png\n- /tmp/b.pdf',
    );
  });

  test('files alone still say something', () => {
    expect(withAttached('  ', ['/tmp/a.png'])).toBe(
      'Look at the attached files.\n\nAttached (open each path):\n- /tmp/a.png',
    );
  });

  test('no files leave the words as they are', () => {
    expect(withAttached(' fix it ', [])).toBe('fix it');
    expect(withAttached('', [])).toBe('');
  });
});

describe('nameOf', () => {
  test('a name with an extension is kept, a path gives its last part', () => {
    expect(nameOf('report.pdf', 'application/pdf', 'file')).toBe('report.pdf');
    expect(nameOf('/var/mobile/IMG_0001.HEIC', 'image/heic', 'photo')).toBe('IMG_0001.HEIC');
  });

  test('without one, the type gives the extension', () => {
    expect(nameOf(null, 'image/png', 'pasted')).toBe('pasted.png');
    expect(nameOf('photo', 'image/jpeg', 'x')).toBe('photo.jpg');
    expect(nameOf(undefined, 'application/x-unknown', 'file')).toBe('file');
  });
});

describe('bytes of base64', () => {
  test('a data URL and plain base64 give the same bytes', () => {
    const plain = new Uint8Array(bytesOfBase64('AAEC/w=='));
    expect([...plain]).toEqual([0, 1, 2, 255]);
    const url = new Uint8Array(bytesOfBase64('data:image/png;base64,AAEC/w=='));
    expect([...url]).toEqual([0, 1, 2, 255]);
    expect(mimeOfDataUrl('data:image/png;base64,AAEC')).toBe('image/png');
    expect(mimeOfDataUrl('AAEC')).toBeUndefined();
  });
});

test('weights read as people say them', () => {
  expect(weight(900)).toBe('900 B');
  expect(weight(2048)).toBe('2 KB');
  expect(weight(5 * 1024 * 1024 + 200_000)).toBe('5.2 MB');
});

describe('splitAttached', () => {
  test('reads back what withAttached wrote', () => {
    expect(splitAttached(withAttached('see', ['/tmp/a.png', '/tmp/b c.pdf']))).toEqual({
      text: 'see',
      files: ['/tmp/a.png', '/tmp/b c.pdf'],
    });
    expect(splitAttached(withAttached('', ['/tmp/a.png']))).toEqual({ text: '', files: ['/tmp/a.png'] });
  });

  test('a message without files is its words', () => {
    expect(splitAttached('just words')).toEqual({ text: 'just words', files: [] });
  });
});

test('a kept file is labelled with its own name', () => {
  expect(fileLabel('/tmp/leon-king-attachments/phone/c/1761475200000-my-photo.png')).toBe('my-photo.png');
  expect(fileLabel('/home/ada/notes.md')).toBe('notes.md');
});
