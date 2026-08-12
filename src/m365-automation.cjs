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

  function normalizePromptText(value) {
    return String(value || '')
      .replace(/\r\n?/g, '\n')
      .replace(/\u00a0/g, ' ')
      .replace(/[\u200b-\u200d\u2060\ufeff]/g, '')
      .trim();
  }

  function classifyComposerPrompt(prompt, composerText) {
    const expected = normalizePromptText(prompt);
    const actual = normalizePromptText(composerText);
    const occurrences = expected
      ? actual.split(expected).length - 1
      : 0;

    if (!actual) {
      return {
        status: 'pending',
        expectedLength: expected.length,
        actualLength: 0,
        occurrences,
      };
    }
    if (actual === expected) {
      return {
        status: 'ready',
        expectedLength: expected.length,
        actualLength: actual.length,
        occurrences: 1,
      };
    }
    return {
      status: occurrences > 1 ? 'duplicate' : 'mismatch',
      expectedLength: expected.length,
      actualLength: actual.length,
      occurrences,
    };
  }

  function normalizeCodeLanguage(value) {
    const language = normalizeText(value)
      .replace(/^language\s*[:=-]?\s*/i, '')
      .toLowerCase();
    const aliases = {
      'c#': 'csharp',
      'c++': 'cpp',
      'f#': 'fsharp',
      'objective-c': 'objectivec',
      'shell script': 'shell',
    };
    const normalized = aliases[language] || language.replace(/\s+/g, '');
    return /^[a-z0-9+#._-]{1,30}$/.test(normalized) ? normalized : '';
  }

  function normalizeCodeText(value) {
    return String(value || '')
      .replace(/\r\n?/g, '\n')
      .replace(/\u00a0/g, ' ')
      .replace(/[\u200b-\u200d\u2060\ufeff]/g, '')
      .replace(/^\n+|\n+$/g, '');
  }

  function isCodeEditor(element) {
    if (!element || typeof element.getAttribute !== 'function') return false;
    if (element.getAttribute('role') !== 'textbox') return false;
    const label = element.getAttribute('aria-label') || '';
    return element.getAttribute('aria-readonly') === 'true'
      || element.getAttribute('aria-multiline') === 'true'
      || /code editor|程式碼編輯器|代码编辑器|コードエディター|코드 편집기/i.test(label);
  }

  function extractCodeBlock(element) {
    if (!element) return { language: '', code: '' };

    const languageBadge = element.querySelector
      ? element.querySelector([
        '#language-badge',
        '[data-testid*="language-badge" i]',
        '[data-testid*="code-language" i]',
        '[class*="language-badge" i]',
      ].join(', '))
      : null;
    const languageCandidates = [
      languageBadge?.getAttribute?.('aria-label'),
      languageBadge?.innerText,
      languageBadge?.textContent,
      ...(isCodeEditor(element)
        ? []
        : Array.from(element.children || [])
          .map((child) => child.innerText || child.textContent || '')),
    ];
    const language = languageCandidates
      .map(normalizeCodeLanguage)
      .find(Boolean) || '';

    const editorSelector = [
      '[role="textbox"][aria-readonly="true"]',
      '[role="textbox"][aria-multiline="true"]',
      '[role="textbox"][aria-label*="code editor" i]',
      '[role="textbox"][aria-label*="程式碼編輯器"]',
      '[role="textbox"][aria-label*="代码编辑器"]',
      '[role="textbox"][aria-label*="コードエディター"]',
      '[role="textbox"][aria-label*="코드 편집기"]',
      '[data-testid*="code-editor" i]',
      'textarea',
    ].join(', ');
    const editor = isCodeEditor(element)
      ? element
      : element.querySelector?.(editorSelector);
    const lineScope = editor || element;
    const indexedLines = Array.from(
      lineScope.querySelectorAll?.('[data-line-index]') || [],
    )
      .map((line, position) => ({
        index: Number.parseInt(line.getAttribute?.('data-line-index') || '', 10),
        position,
        text: line.innerText ?? line.textContent ?? '',
      }))
      .sort((left, right) => {
        const leftIndex = Number.isFinite(left.index) ? left.index : left.position;
        const rightIndex = Number.isFinite(right.index) ? right.index : right.position;
        return leftIndex - rightIndex;
      });

    let code = '';
    if (indexedLines.length > 0) {
      const seen = new Set();
      code = indexedLines
        .filter((line) => {
          const key = Number.isFinite(line.index) ? line.index : line.position;
          if (seen.has(key)) return false;
          seen.add(key);
          return true;
        })
        .map((line) => line.text)
        .join('\n');
    } else {
      const viewLines = Array.from(
        lineScope.querySelectorAll?.('.view-lines .view-line, .view-line') || [],
      );
      if (viewLines.length > 0) {
        code = viewLines
          .map((line) => line.innerText ?? line.textContent ?? '')
          .join('\n');
      } else if (typeof editor?.value === 'string' && editor.value) {
        code = editor.value;
      } else {
        const codeElement = lineScope.querySelector?.('pre code, code');
        code = codeElement?.textContent
          ?? editor?.innerText
          ?? editor?.textContent
          ?? lineScope.innerText
          ?? lineScope.textContent
          ?? '';
      }
    }

    return {
      language,
      code: normalizeCodeText(code),
    };
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
      // Overflowed attachment chips remain in the DOM with display:none.
      ? Array.from(scope.querySelectorAll(selectors))
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
    classifyComposerPrompt,
    extractCodeBlock,
    filterGeneratedImageDescriptors,
    generatedImagesFromLatestTurn,
    inspectAttachment,
    normalizeText,
    normalizePromptText,
  };
});
