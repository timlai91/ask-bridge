'use strict';

(function initializeM365Automation(root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) {
    module.exports = api;
  }
  root.AskBridgeM365Automation = api;
})(typeof globalThis === 'object' ? globalThis : this, function createM365Automation() {
  function normalizeText(value) {
    return String(value || '').normalize('NFKC').replace(/\s+/g, ' ').trim();
  }

  function acceptsFile(accept, file) {
    const rules = String(accept || '')
      .split(',')
      .map((rule) => rule.trim().toLowerCase())
      .filter(Boolean);
    if (rules.length === 0) return true;

    const name = String(file.name || '').toLowerCase();
    const type = String(file.type || '').toLowerCase();
    const topLevel = type.split('/')[0];
    return rules.some((rule) => (
      rule === '*/*'
      || rule === type
      || (rule.startsWith('.') && name.endsWith(rule))
      || (rule.endsWith('/*') && topLevel && rule === `${topLevel}/*`)
    ));
  }

  function classifyAttachmentSignals(input) {
    if (input.authRedirect) {
      return { status: 'authentication', detail: 'Microsoft sign-in is required' };
    }

    const fileName = normalizeText(input.fileName).toLowerCase();
    const alerts = (input.alerts || []).map(normalizeText).filter(Boolean);
    const policyPattern = /\b(?:dlp|policy|organization|administrator|blocked|not allowed|restricted)\b|組織原則|组织策略|系統管理員|管理员|不允許|不允许|已封鎖|已阻止/i;
    const errorPattern = /\b(?:failed|error|rejected|unsupported|not supported|file is empty|couldn'?t upload|cannot upload)\b|上傳失敗|上传失败|不支援|不支持|遭拒/i;
    const pendingPattern = /\b(?:uploading|upload in progress|scanning|processing|preparing)\b|\b\d{1,3}%\b|上傳中|上传中|掃描中|扫描中|處理中|处理中/i;

    const policyAlert = alerts.find((text) => policyPattern.test(text));
    if (policyAlert) {
      return { status: 'policy', detail: policyAlert.slice(0, 200) };
    }
    const errorAlert = alerts.find((text) => errorPattern.test(text));
    if (errorAlert) {
      return { status: 'error', detail: errorAlert.slice(0, 200) };
    }

    const entries = (input.entries || []).map((entry) => ({
      ...entry,
      text: normalizeText(entry.text),
    }));
    const related = entries.filter((entry) => (
      fileName && entry.text.toLowerCase().includes(fileName)
    ));
    const policyEntry = related.find((entry) => policyPattern.test(entry.text));
    if (policyEntry) {
      return { status: 'policy', detail: policyEntry.text.slice(0, 200) };
    }
    const failed = related.find((entry) => errorPattern.test(entry.text) || entry.error);
    if (failed) {
      return { status: 'error', detail: failed.text.slice(0, 200) || 'attachment rejected' };
    }
    const pending = related.find((entry) => (
      entry.busy
      || entry.progress
      || pendingPattern.test(entry.text)
    ));
    if (pending) {
      return { status: 'pending', detail: pending.text.slice(0, 200) };
    }
    if (related.length > 0) {
      return {
        status: 'done',
        detail: related[0].text.slice(0, 200),
        removable: related.some((entry) => entry.removable),
      };
    }

    const unrelatedPending = entries.some((entry) => entry.busy || entry.progress);
    return unrelatedPending
      ? { status: 'pending', detail: 'attachment processing is still active' }
      : { status: 'not-found', detail: 'attachment chip not found' };
  }

  function inspectAttachment(fileName) {
    const isVisible = (element) => {
      if (!element) return false;
      if (typeof element.getClientRects !== 'function') return true;
      return element.getClientRects().length > 0;
    };
    const scope = document.querySelector('#m365-chat-input-shared-container');
    const selectors = [
      '[data-testid*="attachment" i]',
      '[data-testid*="upload" i]',
      '[class*="attachment" i]',
      '[class*="file-chip" i]',
      '[role="progressbar"]',
    ].join(', ');
    const elements = scope
      ? Array.from(scope.querySelectorAll(selectors)).filter(isVisible)
      : [];
    const entries = elements.map((element) => {
      const removeButton = element.querySelector
        ? element.querySelector(
          [
            'button[aria-label*="remove" i]',
            'button[aria-label*="delete" i]',
            'button[aria-label*="移除"]',
            'button[aria-label*="刪除"]',
            'button[aria-label*="删除"]',
            'button[title*="remove" i]',
            'button[title*="delete" i]',
            'button[title*="移除"]',
            'button[title*="刪除"]',
            'button[title*="删除"]',
          ].join(', '),
        )
        : null;
      return {
        text: element.innerText || element.textContent || element.getAttribute('aria-label') || '',
        busy: element.getAttribute('aria-busy') === 'true',
        progress: element.getAttribute('role') === 'progressbar'
          || element.getAttribute('data-state') === 'loading',
        error: element.getAttribute('aria-invalid') === 'true'
          || element.getAttribute('data-state') === 'error',
        removable: Boolean(removeButton),
      };
    });
    const alerts = Array.from(document.querySelectorAll('[role="alert"], [aria-live="assertive"]'))
      .filter(isVisible)
      .map((element) => element.innerText || element.textContent || '');
    return classifyAttachmentSignals({
      fileName,
      authRedirect: /^(?:login\.microsoftonline\.com|login\.live\.com)$/i.test(
        window.location.hostname,
      ),
      entries,
      alerts,
    });
  }

  function filterGeneratedImageDescriptors(descriptors) {
    const seen = new Set();
    return (descriptors || []).filter((descriptor) => {
      const src = String(descriptor.src || '');
      if (!/^(?:https?:|blob:|data:image\/)/i.test(src)) return false;
      if (seen.has(src)) return false;
      if (descriptor.excluded || !descriptor.generatedContainer) return false;
      if ((descriptor.width || 0) > 0 && descriptor.width < 100) return false;
      if ((descriptor.height || 0) > 0 && descriptor.height < 100) return false;
      seen.add(src);
      return true;
    });
  }

  function generatedImagesFromLatestTurn(latestTurn) {
    if (!latestTurn) return [];
    const excludedSelector = [
      '[data-testid*="avatar" i]',
      '[data-testid*="citation" i]',
      '[data-testid*="source" i]',
      '[data-testid*="attachment" i]',
      '[data-testid*="user" i]',
      '[class*="avatar" i]',
      '[class*="citation" i]',
      '[class*="source-card" i]',
      '[class*="attachment" i]',
    ].join(', ');
    const generatedSelector = [
      '[data-testid*="generated-image" i]',
      '[data-testid*="image-generation" i]',
      '[class*="generated-image" i]',
      '[class*="image-generation" i]',
    ].join(', ');
    const descriptors = Array.from(latestTurn.querySelectorAll('img')).map((image) => {
      const alt = normalizeText(image.alt);
      const surrounding = image.closest ? image.closest('figure, [role="group"], div') : null;
      const downloadButton = surrounding && surrounding.querySelector
        ? surrounding.querySelector(
          'button[aria-label*="download" i], a[download], a[aria-label*="download" i]',
        )
        : null;
      return {
        src: image.currentSrc || image.src || image.getAttribute('src') || '',
        alt,
        width: image.naturalWidth || image.width || 0,
        height: image.naturalHeight || image.height || 0,
        excluded: Boolean(image.closest && image.closest(excludedSelector))
          || /\b(?:avatar|profile|icon|logo|citation|source)\b/i.test(alt),
        generatedContainer: Boolean(image.closest && image.closest(generatedSelector))
          || Boolean(downloadButton)
          || /^(?:generated image|已生成影像|產生的圖片|生成的图像)$/i.test(alt)
          || /generated image|已生成影像|產生的圖片|生成的图像/i.test(
            image.parentElement?.getAttribute?.('aria-label') || '',
          ),
      };
    });
    return filterGeneratedImageDescriptors(descriptors);
  }

  return {
    acceptsFile,
    classifyAttachmentSignals,
    filterGeneratedImageDescriptors,
    generatedImagesFromLatestTurn,
    inspectAttachment,
    normalizeText,
  };
});
