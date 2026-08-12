'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const {
  acceptsFile,
  classifyAttachmentSignals,
  filterGeneratedImageDescriptors,
} = require('../src/m365-automation.cjs');

test('matches M365 accept rules by MIME, wildcard, and extension', () => {
  assert.equal(acceptsFile('application/pdf', { name: 'brief.pdf', type: 'application/pdf' }), true);
  assert.equal(acceptsFile('image/*', { name: 'photo.png', type: 'image/png' }), true);
  assert.equal(acceptsFile('.docx,.txt', {
    name: '報告.DOCX',
    type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  }), true);
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
