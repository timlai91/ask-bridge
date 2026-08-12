'use strict';

(function initializeModelSelection(root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) {
    module.exports = api;
  }
  root.AskBridgeModelSelection = api;
})(typeof globalThis === 'object' ? globalThis : this, function createModelSelection() {
  function normalizeLabel(value) {
    return String(value || '')
      .normalize('NFKC')
      .toLowerCase()
      .replace(/[^\p{Letter}\p{Number}]+/gu, '');
  }

  function parseMenuEntry(input) {
    const source = typeof input === 'string' ? { text: input } : (input || {});
    const lines = String(source.text || '')
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    const selectedPrefix = /^(?:selected|已選取|已选中)\s*[:：]?\s*/i;
    const textMarksSelected = Boolean(lines[0] && selectedPrefix.test(lines[0]));
    if (textMarksSelected) {
      lines[0] = lines[0].replace(selectedPrefix, '').trim();
      if (!lines[0]) lines.shift();
    }
    const badgeText = String(source.badgeText || '').trim();
    const secondaryLines = lines.slice(1).filter((line) => line !== badgeText);
    const selectedValues = [
      source.ariaChecked,
      source.ariaSelected,
      source.dataSelected,
      source.dataState,
    ].map((value) => String(value || '').toLowerCase());

    return {
      primaryLabel: lines[0] || '',
      secondaryDescription: secondaryLines.join(' '),
      badgeText,
      selected: textMarksSelected
        || selectedValues.some((value) => ['true', 'checked', 'selected'].includes(value)),
      unavailable: Boolean(source.disabled)
        || /^(?:true|disabled)$/i.test(String(source.ariaDisabled || ''))
        || /\b(?:locked|unavailable|not available)\b|已鎖定|已锁定|無法使用|无法使用/i.test(
          lines.join(' '),
        ),
      element: source.element,
    };
  }

  function labelsMatch(label, aliases) {
    const normalized = normalizeLabel(label);
    return Boolean(normalized) && aliases.some((alias) => normalizeLabel(alias) === normalized);
  }

  function findMatchingEntry(entries, aliases) {
    return entries.find((entry) => labelsMatch(entry.primaryLabel, aliases));
  }

  function availablePrimaryLabels(entries) {
    return [...new Set(entries.map((entry) => entry.primaryLabel).filter(Boolean))];
  }

  function selectionVerified(entry, pickerText, verificationAliases) {
    return Boolean(
      (entry && entry.selected)
      || labelsMatch(parseMenuEntry(pickerText).primaryLabel, verificationAliases),
    );
  }

  function textOf(element) {
    return element
      ? (element.innerText || element.textContent || element.getAttribute('aria-label') || '')
      : '';
  }

  function splitM365Label(entry, kind) {
    const observedLabels = kind === 'reasoning'
      ? ['Quick response', 'Think deeper', '快速回應', '快速回应', '深度思考', 'Auto', '自動']
      : ['GPT 5.6', 'GPT 5.5', 'Sonnet', 'Opus'];
    const primaryLabel = observedLabels.find((label) => (
      normalizeLabel(entry.primaryLabel).startsWith(normalizeLabel(label))
    ));
    if (!primaryLabel) return entry;
    const secondaryDescription = entry.primaryLabel.slice(primaryLabel.length).trim();
    return {
      ...entry,
      primaryLabel,
      secondaryDescription: [
        secondaryDescription,
        entry.secondaryDescription,
      ].filter(Boolean).join(' '),
    };
  }

  function entryFromElement(element, config = {}) {
    const badge = element.querySelector
      ? element.querySelector('[data-testid*="badge"], [class*="badge"]')
      : null;
    const selectedDescendant = element.querySelector
      ? element.querySelector(
        '[aria-checked="true"], [aria-selected="true"], [data-selected="true"], [data-state="checked"]',
      )
      : null;
    const entry = parseMenuEntry({
      text: textOf(element),
      ariaChecked: element.getAttribute('aria-checked')
        || (selectedDescendant && selectedDescendant.getAttribute('aria-checked')),
      ariaSelected: element.getAttribute('aria-selected')
        || (selectedDescendant && selectedDescendant.getAttribute('aria-selected')),
      dataSelected: element.getAttribute('data-selected')
        || (selectedDescendant && selectedDescendant.getAttribute('data-selected')),
      dataState: element.getAttribute('data-state')
        || (selectedDescendant && selectedDescendant.getAttribute('data-state')),
      badgeText: textOf(badge),
      disabled: Boolean(element.disabled),
      ariaDisabled: element.getAttribute('aria-disabled'),
      element,
    });
    return config.provider === 'm365'
      ? splitM365Label(entry, config.kind)
      : entry;
  }

  function visibleElements(selector) {
    return Array.from(document.querySelectorAll(selector)).filter((element) => {
      if (typeof element.getClientRects !== 'function') return true;
      return element.getClientRects().length > 0;
    });
  }

  function dispatchClick(element) {
    if (typeof PointerEvent === 'function') {
      const pointerOptions = {
        bubbles: true,
        isPrimary: true,
        pointerId: 1,
        pointerType: 'mouse',
      };
      element.dispatchEvent(new PointerEvent('pointerdown', pointerOptions));
      element.dispatchEvent(new PointerEvent('pointerup', pointerOptions));
    } else {
      element.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
      element.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
    }
    element.click();
  }

  function dispatchHover(element) {
    if (typeof PointerEvent === 'function') {
      const pointerOptions = {
        bubbles: true,
        isPrimary: true,
        pointerId: 1,
        pointerType: 'mouse',
      };
      element.dispatchEvent(new PointerEvent('pointerenter', pointerOptions));
      element.dispatchEvent(new PointerEvent('pointermove', pointerOptions));
    } else {
      element.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
      element.dispatchEvent(new MouseEvent('mousemove', { bubbles: true }));
    }
    element.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
  }

  function findPicker(provider) {
    if (provider === 'chatgpt') {
      return document.querySelector('button.__composer-pill');
    }
    if (provider === 'gemini') {
      return visibleElements('button').find((button) => (
        /模式挑選器|model picker|mode picker/i.test([
          button.getAttribute('aria-label'),
          button.textContent,
        ].filter(Boolean).join(' '))
      ));
    }
    if (provider === 'm365') {
      const exact = visibleElements('#gptModeSwitcher').find((button) => !button.closest('nav'));
      if (exact) return exact;

      return visibleElements('main button[aria-haspopup="menu"]').find((button) => (
        !button.closest('nav')
        && /model selector|模型選擇器|模型选择器/i.test([
          button.getAttribute('aria-label'),
          button.textContent,
        ].filter(Boolean).join(' '))
      ));
    }
    return undefined;
  }

  function m365EntryKind(entry) {
    const role = entry.element.getAttribute('role');
    if (role !== 'menuitemradio' && role !== 'option') return undefined;
    const menu = entry.element.closest ? entry.element.closest('[role="menu"], [role="listbox"]') : null;
    if (!menu || !menu.querySelector) return undefined;
    return menu.querySelector('[role="menuitem"][aria-haspopup="menu"]')
      ? 'reasoning'
      : 'model';
  }

  function isCandidateEntry(entry, config) {
    if (entry.element.getAttribute('aria-haspopup') === 'menu') return false;
    if (config.provider !== 'm365') return true;
    return m365EntryKind(entry) === config.kind;
  }

  async function findProviderOption(config, menuSelector, available, sleep) {
    const visited = new Set();
    const canTraverseNested = config.provider === 'chatgpt'
      || (config.provider === 'm365' && config.kind === 'model');
    const maxDepth = canTraverseNested ? 6 : 1;

    for (let depth = 0; depth < maxDepth; depth += 1) {
      const elements = visibleElements(menuSelector);
      const entries = elements.map((element) => entryFromElement(element, config));
      const leaves = entries.filter((entry) => isCandidateEntry(entry, config));
      leaves.forEach((entry) => available.add(entry.primaryLabel));
      const chosen = findMatchingEntry(leaves, config.targetAliases);
      if (chosen || !canTraverseNested) return chosen;

      const triggers = entries.filter(
        (entry) => entry.element.getAttribute('aria-haspopup') === 'menu',
      );
      const trigger = triggers.find((entry) => {
        const key = `${normalizeLabel(entry.primaryLabel)}|${entry.element.getAttribute('aria-label') || ''}`;
        return !visited.has(key);
      });
      if (!trigger) return undefined;

      const key = `${normalizeLabel(trigger.primaryLabel)}|${trigger.element.getAttribute('aria-label') || ''}`;
      visited.add(key);
      dispatchHover(trigger.element);
      trigger.element.click();
      await sleep(750);
    }

    return undefined;
  }

  function captureM365ComposerState() {
    const composer = document.querySelector(
      '#m365-chat-editor-target-element, #m365-chat-input-shared-container [role="textbox"][contenteditable="true"]',
    );
    const scope = document.querySelector('#m365-chat-input-shared-container');
    const attachmentSelector = [
      '[data-testid*="attachment" i]',
      '[data-testid*="upload" i]',
      '[class*="attachment" i]',
      '[class*="file-chip" i]',
    ].join(', ');
    const attachments = scope
      ? visibleElements(attachmentSelector)
        .filter((element) => scope.contains(element))
        .map((element) => textOf(element).replace(/\s+/g, ' ').trim())
        .filter(Boolean)
      : [];
    return {
      composerText: textOf(composer).trim(),
      attachments: [...new Set(attachments)],
    };
  }

  function composerStatePreserved(before, after) {
    return before.composerText === after.composerText
      && JSON.stringify(before.attachments) === JSON.stringify(after.attachments);
  }

  function m365AuthRedirected() {
    return typeof window === 'object'
      && /^(?:login\.microsoftonline\.com|login\.live\.com)$/i.test(
        window.location?.hostname || '',
      );
  }

  async function selectProviderOption(config) {
    const sleep = config.sleep
      || ((milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)));
    const menuSelector = '[role="menuitem"], [role="menuitemradio"], [role="option"]';
    const available = new Set();

    document.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'Escape',
      keyCode: 27,
      bubbles: true,
    }));
    await sleep(250);

    if (config.provider === 'm365' && m365AuthRedirected()) {
      return {
        ok: false,
        error: 'authentication: Microsoft sign-in is required during selection',
        available: [],
      };
    }

    let picker;
    if (config.provider === 'chatgpt' || config.provider === 'm365') {
      for (let attempt = 0; attempt < 20; attempt += 1) {
        picker = findPicker(config.provider);
        if (picker) break;
        await sleep(250);
      }
    } else if (config.provider === 'gemini') {
      picker = findPicker(config.provider);
    }

    if (!picker) {
      if (config.provider === 'm365' && /\/chat\/conversation\//i.test(window.location.pathname)) {
        return {
          ok: false,
          error: 'existing conversation does not allow model or reasoning changes; start with --new',
          available: [],
        };
      }
      if (config.provider === 'm365') {
        return {
          ok: false,
          error: `${config.kind || 'selection'} picker not found; this may be a Microsoft 365 UI rollout`,
          available: [],
        };
      }
      return { ok: false, error: `${config.provider} picker not found`, available: [] };
    }
    if (picker.disabled || picker.getAttribute('aria-disabled') === 'true') {
      return {
        ok: false,
        error: `${config.provider} picker is locked or unavailable`,
        available: [],
      };
    }

    const composerState = config.provider === 'm365'
      ? captureM365ComposerState()
      : undefined;
    dispatchClick(picker);
    await sleep(800);

    if (config.provider === 'm365' && m365AuthRedirected()) {
      return {
        ok: false,
        error: 'authentication: Microsoft sign-in expired during selection',
        available: [],
      };
    }

    const chosen = await findProviderOption(config, menuSelector, available, sleep);

    if (!chosen) {
      document.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Escape',
        keyCode: 27,
        bubbles: true,
      }));
      return {
        ok: false,
        error: 'option not found',
        available: [...available].filter(Boolean),
      };
    }

    if (chosen.unavailable) {
      document.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Escape',
        keyCode: 27,
        bubbles: true,
      }));
      return {
        ok: false,
        error: `${chosen.primaryLabel} is locked or unavailable`,
        available: [...available].filter(Boolean),
      };
    }

    if (chosen.selected) {
      document.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Escape',
        keyCode: 27,
        bubbles: true,
      }));
      return {
        ok: true,
        selected: chosen.primaryLabel,
        unchanged: true,
        available: [...available].filter(Boolean),
      };
    }

    chosen.element.click();
    await sleep(600);

    if (config.provider === 'm365' && m365AuthRedirected()) {
      return {
        ok: false,
        error: 'authentication: Microsoft sign-in expired during selection',
        available: [...available].filter(Boolean),
      };
    }

    picker = findPicker(config.provider) || picker;
    let currentEntry = entryFromElement(chosen.element, config);
    let verified = selectionVerified(
      currentEntry,
      textOf(picker),
      config.verificationAliases,
    );

    if (!verified) {
      picker = findPicker(config.provider) || picker;
      dispatchClick(picker);
      await sleep(500);
      currentEntry = await findProviderOption(config, menuSelector, available, sleep);
      verified = selectionVerified(
        currentEntry,
        textOf(picker),
        config.verificationAliases,
      );
    }

    document.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'Escape',
      keyCode: 27,
      bubbles: true,
    }));

    if (composerState && !composerStatePreserved(composerState, captureM365ComposerState())) {
      return {
        ok: false,
        error: 'selection changed the composer prompt or attachments',
        available: [...available].filter(Boolean),
      };
    }

    if (!verified) {
      return {
        ok: false,
        error: `selection could not be verified for ${chosen.primaryLabel}`,
        available: [...available].filter(Boolean),
      };
    }

    return {
      ok: true,
      selected: chosen.primaryLabel,
      available: [...available].filter(Boolean),
    };
  }

  return {
    availablePrimaryLabels,
    findMatchingEntry,
    normalizeLabel,
    parseMenuEntry,
    selectProviderOption,
    selectionVerified,
  };
});
