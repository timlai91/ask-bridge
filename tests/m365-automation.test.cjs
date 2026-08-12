'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const {
  acceptsFile,
  classifyAttachmentSignals,
  classifyComposerPrompt,
  extractCodeBlock,
  filterGeneratedImageDescriptors,
  inspectAttachment,
} = require('../src/m365-automation.cjs');

test('matches M365 accept rules by MIME, wildcard, and extension', () => {
  assert.equal(acceptsFile('application/pdf', { name: 'brief.pdf', type: 'application/pdf' }), true);
  assert.equal(acceptsFile('image/*', { name: 'photo.png', type: 'image/png' }), true);
  assert.equal(acceptsFile('.docx,.txt', {
    name: '報告.DOCX',
    type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  }), true);
  assert.equal(acceptsFile('.csv', { name: 'data.csv', type: 'text/csv' }), true);
  assert.equal(acceptsFile('', { name: 'sample.custom', type: 'application/octet-stream' }), true);
  assert.equal(acceptsFile('.pdf', { name: 'brief.txt', type: 'text/plain' }), false);
});

test('classifies attachment done, pending, error, policy, and authentication states', () => {
  assert.deepEqual(
    classifyAttachmentSignals({
      fileName: 'brief.pdf',
      entries: [{ text: 'brief.pdf', removable: true }],
    }),
    { status: 'done', detail: 'brief.pdf', removable: true },
  );
  assert.equal(classifyAttachmentSignals({
    fileName: 'brief.pdf',
    entries: [{ text: 'brief.pdf Uploading', busy: true }],
  }).status, 'pending');
  assert.equal(classifyAttachmentSignals({
    fileName: 'brief.pdf',
    entries: [{ text: 'brief.pdf upload in progress, 40%' }],
  }).status, 'pending');
  assert.equal(classifyAttachmentSignals({
    fileName: 'brief.pdf',
    entries: [{ text: 'brief.pdf upload failed', error: true }],
  }).status, 'error');
  assert.equal(classifyAttachmentSignals({
    fileName: 'brief.pdf',
    alerts: ['This file type is not supported. Upload a different file.'],
  }).status, 'error');
  assert.equal(classifyAttachmentSignals({
    fileName: 'brief.pdf',
    alerts: ['Upload failed - file is empty.'],
  }).status, 'error');
  assert.equal(classifyAttachmentSignals({
    fileName: 'brief.pdf',
    alerts: ['Your organization DLP policy blocked this file.'],
  }).status, 'policy');
  assert.equal(classifyAttachmentSignals({
    fileName: 'brief.pdf',
    authRedirect: true,
  }).status, 'authentication');
});

test('detects M365 attachment chips hidden by the overflow menu', () => {
  const originalDocument = globalThis.document;
  const originalWindow = globalThis.window;
  const hiddenAttachment = {
    innerText: 'hidden.cs',
    textContent: 'hidden.cs',
    getClientRects: () => [],
    getAttribute: () => null,
    querySelector: () => ({}),
  };
  const scope = {
    querySelectorAll: () => [hiddenAttachment],
  };

  globalThis.document = {
    querySelector: () => scope,
    querySelectorAll: () => [],
  };
  globalThis.window = {
    location: { hostname: 'm365.cloud.microsoft' },
  };

  try {
    assert.deepEqual(inspectAttachment('hidden.cs'), {
      status: 'done',
      detail: 'hidden.cs',
      removable: true,
    });
  } finally {
    if (originalDocument === undefined) delete globalThis.document;
    else globalThis.document = originalDocument;
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
  }
});

test('requires one exact M365 prompt before submission', () => {
  assert.equal(classifyComposerPrompt('Explain this file', '').status, 'pending');
  assert.equal(
    classifyComposerPrompt('Explain this file', 'Explain this file').status,
    'ready',
  );
  assert.deepEqual(
    classifyComposerPrompt(
      'Explain this file',
      'Explain this fileExplain this file',
    ),
    {
      status: 'duplicate',
      expectedLength: 17,
      actualLength: 34,
      occurrences: 2,
    },
  );
  assert.equal(
    classifyComposerPrompt(
      'line 1\nline 2',
      'line 1\r\nline 2\u200b\u200c\u200d\u2060\ufeff',
    ).status,
    'ready',
  );
});

test('extracts localized virtualized M365 code blocks without line numbers', () => {
  const lines = [
    {
      innerText: 'private readonly IFileDiscoveryService _fileDiscovery;',
      getAttribute: (name) => (name === 'data-line-index' ? '0' : null),
    },
    {
      innerText: '',
      getAttribute: (name) => (name === 'data-line-index' ? '1' : null),
    },
    {
      innerText: 'await ProtectFileAsync(file, ct);',
      getAttribute: (name) => (name === 'data-line-index' ? '2' : null),
    },
  ];
  const editor = {
    innerText: '1\nprivate readonly IFileDiscoveryService _fileDiscovery;\n2\n\n3\nawait ProtectFileAsync(file, ct);',
    getAttribute: (name) => ({
      role: 'textbox',
      'aria-label': '程式碼編輯器',
      'aria-readonly': 'true',
      'aria-multiline': 'true',
    })[name] || null,
    querySelector: () => null,
    querySelectorAll: (selector) => (selector === '[data-line-index]' ? lines : []),
  };
  const languageBadge = {
    innerText: 'C#',
    getAttribute: (name) => (name === 'aria-label' ? 'C#' : null),
  };
  const block = {
    children: [],
    getAttribute: () => null,
    querySelector: (selector) => (
      selector.includes('#language-badge') ? languageBadge : editor
    ),
    querySelectorAll: () => [],
  };

  assert.deepEqual(extractCodeBlock(block), {
    language: 'csharp',
    code: [
      'private readonly IFileDiscoveryService _fileDiscovery;',
      '',
      'await ProtectFileAsync(file, ct);',
    ].join('\n'),
  });
  assert.deepEqual(extractCodeBlock(editor), {
    language: '',
    code: [
      'private readonly IFileDiscoveryService _fileDiscovery;',
      '',
      'await ProtectFileAsync(file, ct);',
    ].join('\n'),
  });
});

test('keeps only generated images and excludes UI or user attachment images', () => {
  const descriptors = [
    { src: 'https://example.test/generated.png', width: 1024, height: 1024, generatedContainer: true },
    { src: 'blob:generated', width: 512, height: 512, generatedContainer: true },
    { src: 'data:image/png;base64,AA==', width: 256, height: 256, generatedContainer: true },
    { src: 'https://example.test/avatar.png', width: 256, height: 256, generatedContainer: true, excluded: true },
    { src: 'https://example.test/citation.png', width: 256, height: 256, generatedContainer: false },
    { src: 'https://example.test/icon.png', width: 32, height: 32, generatedContainer: true },
    { src: 'https://example.test/generated.png', width: 1024, height: 1024, generatedContainer: true },
  ];

  assert.deepEqual(
    filterGeneratedImageDescriptors(descriptors).map((descriptor) => descriptor.src),
    [
      'https://example.test/generated.png',
      'blob:generated',
      'data:image/png;base64,AA==',
    ],
  );
});
