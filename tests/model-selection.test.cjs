'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const {
  availablePrimaryLabels,
  findMatchingEntry,
  parseMenuEntry,
  selectProviderOption,
  selectionVerified,
} = require('../src/model-selection.cjs');

function fixture(text, attributes = {}) {
  return parseMenuEntry({
    text,
    ariaChecked: attributes.ariaChecked,
    dataState: attributes.dataState,
    badgeText: attributes.badgeText,
  });
}

test('matches ChatGPT reasoning by primary label and ignores secondary version text', () => {
  const entries = [
    fixture('即時\n5.5'),
    fixture('中'),
    fixture('高'),
  ];

  assert.equal(findMatchingEntry(entries, ['instant', '即時']).primaryLabel, '即時');
  assert.equal(findMatchingEntry(entries, ['medium', '中', '中等']).primaryLabel, '中');
  assert.equal(findMatchingEntry(entries, ['high', '高']).primaryLabel, '高');
  assert.equal(findMatchingEntry(entries, ['5.5']), undefined);
});

test('matches English ChatGPT reasoning labels', () => {
  const entries = [
    fixture('Instant\n5.5'),
    fixture('Medium'),
    fixture('High'),
  ];

  assert.equal(findMatchingEntry(entries, ['instant', '即時']).primaryLabel, 'Instant');
  assert.equal(findMatchingEntry(entries, ['medium', '中', '中等']).primaryLabel, 'Medium');
  assert.equal(findMatchingEntry(entries, ['high', '高']).primaryLabel, 'High');
});

test('matches Gemini modes exactly without subtitles or badges', () => {
  const entries = [
    fixture('3.5 Flash-Lite\n回覆最快\n新模型', { badgeText: '新模型' }),
    fixture('3.6 Flash\n全方位協助\n新模型', { badgeText: '新模型' }),
    fixture('3.1 Pro\n進階數學與程式設計'),
    fixture('延伸思考\n解決複雜問題'),
  ];

  assert.equal(findMatchingEntry(entries, ['3.5 Flash-Lite']).primaryLabel, '3.5 Flash-Lite');
  assert.equal(findMatchingEntry(entries, ['3.6 Flash']).primaryLabel, '3.6 Flash');
  assert.equal(findMatchingEntry(entries, ['3.1 Pro']).primaryLabel, '3.1 Pro');
  assert.equal(findMatchingEntry(entries, ['extended thinking', '延伸思考']).primaryLabel, '延伸思考');
  assert.equal(findMatchingEntry(entries, ['3.5 Flash']), undefined);
  assert.deepEqual(availablePrimaryLabels(entries), [
    '3.5 Flash-Lite',
    '3.6 Flash',
    '3.1 Pro',
    '延伸思考',
  ]);
});

test('separates selected-state text from the Gemini primary label', () => {
  const selected = fixture('已選取\n3.1 Pro\n進階數學與程式設計');
  const selectedEnglish = fixture('Selected: Extended Thinking\nSolve complex problems');

  assert.equal(selected.primaryLabel, '3.1 Pro');
  assert.equal(selected.selected, true);
  assert.equal(selectedEnglish.primaryLabel, 'Extended Thinking');
  assert.equal(selectedEnglish.selected, true);
});

test('matches English Gemini Extended Thinking label', () => {
  const entries = [
    fixture('3.1 Pro\nAdvanced math and coding'),
    fixture('Extended Thinking\nSolve complex problems'),
  ];

  assert.equal(
    findMatchingEntry(entries, ['extended thinking', '延伸思考']).primaryLabel,
    'Extended Thinking',
  );
});

test('verifies selection through checked state or the picker primary label', () => {
  const checked = fixture('高', { ariaChecked: 'true' });
  const unchecked = fixture('高', { ariaChecked: 'false' });

  assert.equal(selectionVerified(checked, '', ['high', '高']), true);
  assert.equal(selectionVerified(unchecked, '高', ['high', '高']), true);
  assert.equal(selectionVerified(unchecked, '中', ['high', '高']), false);
  assert.equal(selectionVerified(undefined, 'Pro 延伸', ['Pro Extended', 'Pro 延伸']), true);
});

test('revisits ChatGPT nested menus when verifying a selected model', async () => {
  const previousDocument = global.document;
  const previousKeyboardEvent = global.KeyboardEvent;
  const previousMouseEvent = global.MouseEvent;
  const previousPointerEvent = global.PointerEvent;
  const pointerEventTypes = [];
  let menuState = 'closed';
  let hasClicked = false;

  function element(text, attributes = {}, click = () => {}) {
    return {
      innerText: text,
      textContent: text,
      click,
      dispatchEvent() {},
      getAttribute(name) {
        const value = typeof attributes[name] === 'function'
          ? attributes[name]()
          : attributes[name];
        return value === undefined ? null : value;
      },
      getClientRects() {
        return [{}];
      },
      querySelector() {
        return null;
      },
    };
  }

  const picker = element('ChatGPT', {}, () => {
    menuState = menuState === 'closed' ? 'top' : 'closed';
  });
  const submenu = element('更多模型', { 'aria-haspopup': 'menu' }, () => {
    menuState = 'nested';
  });
  const model = element(
    'GPT-5.6 Sol\n適合程式設計',
    {
      'aria-checked': () => (menuState === 'nested' ? 'true' : 'false'),
    },
    () => {
      hasClicked = true;
      menuState = 'closed';
    },
  );
  model.getAttribute = (name) => {
    if (name === 'aria-checked') return hasClicked && menuState === 'nested' ? 'true' : 'false';
    return null;
  };

  global.KeyboardEvent = class KeyboardEvent {};
  global.MouseEvent = class MouseEvent {};
  global.PointerEvent = class PointerEvent {
    constructor(type) {
      pointerEventTypes.push(type);
    }
  };
  global.document = {
    dispatchEvent() {},
    querySelector(selector) {
      return selector === 'button.__composer-pill' ? picker : null;
    },
    querySelectorAll(selector) {
      if (selector === 'button') return [picker];
      if (menuState === 'top') return [submenu];
      if (menuState === 'nested') return [model];
      return [];
    },
  };

  try {
    const result = await selectProviderOption({
      provider: 'chatgpt',
      targetAliases: ['GPT-5.6 Sol'],
      verificationAliases: ['GPT-5.6 Sol'],
      sleep: async () => {},
    });

    assert.equal(result.ok, true);
    assert.equal(result.selected, 'GPT-5.6 Sol');
    assert.deepEqual(pointerEventTypes, [
      'pointerdown',
      'pointerup',
      'pointerenter',
      'pointermove',
      'pointerdown',
      'pointerup',
      'pointerenter',
      'pointermove',
    ]);
  } finally {
    global.document = previousDocument;
    global.KeyboardEvent = previousKeyboardEvent;
    global.MouseEvent = previousMouseEvent;
    global.PointerEvent = previousPointerEvent;
  }
});

test('selects M365 reasoning only from the top-level model switcher menu', async () => {
  const previousDocument = global.document;
  const previousKeyboardEvent = global.KeyboardEvent;
  const previousMouseEvent = global.MouseEvent;
  const previousPointerEvent = global.PointerEvent;
  let menuState = 'closed';
  let selectedLabel = 'Auto';

  function element(text, role, menu, click = () => {}, attributes = {}) {
    return {
      innerText: text,
      textContent: text,
      disabled: false,
      click,
      dispatchEvent() {},
      getAttribute(name) {
        if (name === 'role') return role;
        if (name === 'aria-checked') return selectedLabel === text.split('\n')[0] ? 'true' : 'false';
        return attributes[name] ?? null;
      },
      getClientRects() {
        return [{}];
      },
      querySelector() {
        return null;
      },
      closest(selector) {
        return selector.includes('[role="menu"]') ? menu : null;
      },
    };
  }

  const topMenu = {
    querySelector(selector) {
      return selector === '[role="menuitem"][aria-haspopup="menu"]' ? {} : null;
    },
  };
  const nestedMenu = { querySelector() { return null; } };
  const picker = element('Auto', null, null, () => {
    menuState = menuState === 'closed' ? 'top' : 'closed';
  }, { 'aria-label': 'Model Selector', 'aria-haspopup': 'menu' });
  const navigationPicker = {
    ...picker,
    click() {
      throw new Error('navigation picker must not be clicked');
    },
    closest(selector) {
      return selector === 'nav' ? {} : null;
    },
  };
  const quick = element('Quick response Answers right away', 'menuitemradio', topMenu, () => {
    selectedLabel = 'Quick response';
    picker.innerText = selectedLabel;
    picker.textContent = selectedLabel;
    menuState = 'closed';
  });
  const gptTrigger = element('GPT\nOpenAI', 'menuitem', topMenu, () => {
    menuState = 'nested';
  }, { 'aria-haspopup': 'menu' });
  const nestedModel = element('GPT 5.6 Think deeper', 'menuitemradio', nestedMenu);

  global.KeyboardEvent = class KeyboardEvent {};
  global.MouseEvent = class MouseEvent {};
  global.PointerEvent = class PointerEvent {};
  global.document = {
    dispatchEvent() {},
    querySelector(selector) {
      if (selector.includes('#gptModeSwitcher')) return picker;
      return null;
    },
    querySelectorAll(selector) {
      if (selector === '#gptModeSwitcher') return [navigationPicker, picker];
      if (selector === '[role="menuitem"], [role="menuitemradio"], [role="option"]') {
        if (menuState === 'top') return [quick, gptTrigger];
        if (menuState === 'nested') return [quick, gptTrigger, nestedModel];
      }
      return [];
    },
  };

  try {
    const result = await selectProviderOption({
      provider: 'm365',
      kind: 'reasoning',
      targetAliases: ['quick response'],
      verificationAliases: ['quick response'],
      sleep: async () => {},
    });
    assert.equal(result.ok, true);
    assert.equal(result.selected, 'Quick response');
    assert.deepEqual(result.available, ['Quick response']);
  } finally {
    global.document = previousDocument;
    global.KeyboardEvent = previousKeyboardEvent;
    global.MouseEvent = previousMouseEvent;
    global.PointerEvent = previousPointerEvent;
  }
});

test('selects M365 nested models without treating reasoning subtitles as models', async () => {
  const previousDocument = global.document;
  const previousKeyboardEvent = global.KeyboardEvent;
  const previousMouseEvent = global.MouseEvent;
  const previousPointerEvent = global.PointerEvent;
  let menuState = 'closed';
  let selectedLabel = 'Auto';

  function element(text, role, menu, click = () => {}, attributes = {}) {
    return {
      innerText: text,
      textContent: text,
      disabled: false,
      click,
      dispatchEvent() {},
      getAttribute(name) {
        if (name === 'role') return role;
        if (name === 'aria-checked') return selectedLabel === text.split('\n')[0] ? 'true' : 'false';
        return attributes[name] ?? null;
      },
      getClientRects() {
        return [{}];
      },
      querySelector() {
        return null;
      },
      closest(selector) {
        return selector.includes('[role="menu"]') ? menu : null;
      },
    };
  }

  const topMenu = {
    querySelector(selector) {
      return selector === '[role="menuitem"][aria-haspopup="menu"]' ? {} : null;
    },
  };
  const nestedMenu = { querySelector() { return null; } };
  const picker = element('Auto', null, null, () => {
    menuState = menuState === 'closed' ? 'top' : 'closed';
  }, { 'aria-label': 'Model Selector', 'aria-haspopup': 'menu' });
  const quick = element('Quick response Answers right away', 'menuitemradio', topMenu);
  const gptTrigger = element('GPT\nOpenAI', 'menuitem', topMenu, () => {
    menuState = 'nested';
  }, { 'aria-haspopup': 'menu' });
  const gpt56 = element('GPT 5.6 Think deeper', 'menuitemradio', nestedMenu, () => {
    selectedLabel = 'GPT 5.6';
    picker.innerText = selectedLabel;
    picker.textContent = selectedLabel;
    menuState = 'closed';
  });

  global.KeyboardEvent = class KeyboardEvent {};
  global.MouseEvent = class MouseEvent {};
  global.PointerEvent = class PointerEvent {};
  global.document = {
    dispatchEvent() {},
    querySelector(selector) {
      if (selector.includes('#gptModeSwitcher')) return picker;
      return null;
    },
    querySelectorAll(selector) {
      if (selector === '#gptModeSwitcher') return [picker];
      if (selector === '[role="menuitem"], [role="menuitemradio"], [role="option"]') {
        if (menuState === 'top') return [quick, gptTrigger];
        if (menuState === 'nested') return [quick, gptTrigger, gpt56];
      }
      return [];
    },
  };

  try {
    const result = await selectProviderOption({
      provider: 'm365',
      kind: 'model',
      targetAliases: ['GPT 5.6'],
      verificationAliases: ['GPT 5.6'],
      sleep: async () => {},
    });
    assert.equal(result.ok, true);
    assert.equal(result.selected, 'GPT 5.6');
    assert.deepEqual(result.available, ['GPT 5.6']);
  } finally {
    global.document = previousDocument;
    global.KeyboardEvent = previousKeyboardEvent;
    global.MouseEvent = previousMouseEvent;
    global.PointerEvent = previousPointerEvent;
  }
});

test('reports locked M365 options instead of option not found', () => {
  const locked = parseMenuEntry({
    text: 'Opus\nUnavailable for your organization',
    ariaDisabled: 'true',
  });
  assert.equal(locked.primaryLabel, 'Opus');
  assert.equal(locked.unavailable, true);
});

test('parses M365 primary labels without subtitles or badges', () => {
  const entry = parseMenuEntry({
    text: 'GPT 5.6\nThink deeper\nPremium',
    badgeText: 'Premium',
    ariaChecked: 'true',
  });
  assert.equal(entry.primaryLabel, 'GPT 5.6');
  assert.equal(entry.secondaryDescription, 'Think deeper');
  assert.equal(entry.badgeText, 'Premium');
  assert.equal(entry.selected, true);
});

test('selects zh-TW M365 reasoning labels without matching subtitles', async () => {
  const previousDocument = global.document;
  const previousKeyboardEvent = global.KeyboardEvent;
  const previousMouseEvent = global.MouseEvent;
  const previousPointerEvent = global.PointerEvent;
  let menuOpen = false;

  const topMenu = {
    querySelector(selector) {
      return selector === '[role="menuitem"][aria-haspopup="menu"]' ? {} : null;
    },
  };
  const picker = {
    innerText: '自動',
    textContent: '自動',
    disabled: false,
    click() {
      menuOpen = !menuOpen;
    },
    dispatchEvent() {},
    getAttribute(name) {
      if (name === 'aria-label') return '模型選擇器';
      if (name === 'aria-haspopup') return 'menu';
      return null;
    },
    getClientRects() {
      return [{}];
    },
    querySelector() {
      return null;
    },
    closest() {
      return null;
    },
  };
  const quick = {
    innerText: '快速回應 立即提供解答',
    textContent: '快速回應 立即提供解答',
    disabled: false,
    click() {
      picker.innerText = '快速回應';
      picker.textContent = '快速回應';
      menuOpen = false;
    },
    dispatchEvent() {},
    getAttribute(name) {
      if (name === 'role') return 'menuitemradio';
      if (name === 'aria-checked') return picker.innerText === '快速回應' ? 'true' : 'false';
      return null;
    },
    getClientRects() {
      return [{}];
    },
    querySelector() {
      return null;
    },
    closest(selector) {
      return selector.includes('[role="menu"]') ? topMenu : null;
    },
  };

  global.KeyboardEvent = class KeyboardEvent {};
  global.MouseEvent = class MouseEvent {};
  global.PointerEvent = class PointerEvent {};
  global.document = {
    dispatchEvent() {},
    querySelector(selector) {
      if (selector.includes('#m365-chat-editor-target-element')) return null;
      if (selector === '#m365-chat-input-shared-container') return null;
      return null;
    },
    querySelectorAll(selector) {
      if (selector === '#gptModeSwitcher') return [picker];
      if (
        selector === '[role="menuitem"], [role="menuitemradio"], [role="option"]'
        && menuOpen
      ) return [quick];
      return [];
    },
  };

  try {
    const result = await selectProviderOption({
      provider: 'm365',
      kind: 'reasoning',
      targetAliases: ['quick response', '快速回應'],
      verificationAliases: ['quick response', '快速回應'],
      sleep: async () => {},
    });
    assert.equal(result.ok, true);
    assert.equal(result.selected, '快速回應');
    assert.deepEqual(result.available, ['快速回應']);
  } finally {
    global.document = previousDocument;
    global.KeyboardEvent = previousKeyboardEvent;
    global.MouseEvent = previousMouseEvent;
    global.PointerEvent = previousPointerEvent;
  }
});

test('stops M365 selection immediately on authentication redirect', async () => {
  const previousDocument = global.document;
  const previousWindow = global.window;
  const previousKeyboardEvent = global.KeyboardEvent;
  global.KeyboardEvent = class KeyboardEvent {};
  global.window = { location: { hostname: 'login.microsoftonline.com', pathname: '/authorize' } };
  global.document = {
    dispatchEvent() {},
    querySelector() {
      return null;
    },
    querySelectorAll() {
      return [];
    },
  };

  try {
    const result = await selectProviderOption({
      provider: 'm365',
      kind: 'model',
      targetAliases: ['GPT 5.6'],
      verificationAliases: ['GPT 5.6'],
      sleep: async () => {},
    });
    assert.equal(result.ok, false);
    assert.match(result.error, /^authentication:/);
  } finally {
    global.document = previousDocument;
    global.window = previousWindow;
    global.KeyboardEvent = previousKeyboardEvent;
  }
});
