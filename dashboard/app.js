const state = {
  account: null,
  strategies: [],
  selectedStrategy: null,
  currentTab: "positions",
  search: "",
  snapshotPollTimer: null,
  positionsView: {
    strategy: null,
    rows: [],
    total: 0,
    loading: false,
    error: null,
    requestId: 0,
  },
  openOrdersView: {
    strategy: null,
    rows: [],
    total: 0,
    loading: false,
    error: null,
    requestId: 0,
  },
  settlementsPage: {
    strategy: null,
    range: "all",
    page: 1,
    pageSize: 100,
    total: 0,
    totalPages: 0,
    rows: [],
    loading: false,
    error: null,
    requestId: 0,
  },
};

const HISTORY_RANGE = "all";

const nodes = {
  connectionStatus: document.getElementById("connection-status"),
  portfolioValue: document.getElementById("portfolio-value"),
  portfolioSecondary: document.getElementById("portfolio-secondary"),
  portfolioSideValue: document.getElementById("portfolio-side-value"),
  statusStrip: document.getElementById("status-strip"),
  heroActions: document.getElementById("hero-actions"),
  accountError: document.getElementById("account-error"),
  strategyCount: document.getElementById("strategy-count"),
  strategySwitcher: document.getElementById("strategy-switcher"),
  tableTabs: document.getElementById("table-tabs"),
  searchInput: document.getElementById("search-input"),
  tableHead: document.getElementById("table-head"),
  tableBody: document.getElementById("table-body"),
  tableSummary: document.getElementById("table-summary"),
};

function selectedStrategy() {
  return state.strategies.find((item) => item.strategy === state.selectedStrategy) || null;
}

function latestActivityTime(strategy) {
  return Math.max(strategy?.last_trade_ms || 0, strategy?.last_order_ms || 0, strategy?.last_signal_ms || 0) || null;
}

function setConnectionStatus(label, tone = "neutral") {
  nodes.connectionStatus.textContent = label;
  nodes.connectionStatus.className = `connection-chip ${tone}`;
}

function setAccountError(message, tone = "tone-neutral") {
  nodes.accountError.textContent = message;
  nodes.accountError.className = `hero-error ${tone}`;
}

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }
  return response.json();
}

async function loadSnapshot() {
  try {
    applySnapshot(await fetchJson("/api/snapshot"));
    setConnectionStatus("轮询同步", "connected");
    render();
  } catch (error) {
    setConnectionStatus("轮询失败", "disconnected");
    setAccountError(`快照加载失败: ${error.message || error}`, "tone-negative");
    render();
  }
}

function startSnapshotPolling() {
  if (state.snapshotPollTimer) return;
  state.snapshotPollTimer = window.setInterval(() => {
    void loadSnapshot();
  }, 3000);
}

function stopSnapshotPolling() {
  if (!state.snapshotPollTimer) return;
  window.clearInterval(state.snapshotPollTimer);
  state.snapshotPollTimer = null;
}

function applySnapshot(payload) {
  const previousStrategy = state.selectedStrategy;
  const previousSettled = selectedStrategy()?.settled_count ?? 0;

  state.account = payload.account;
  state.strategies = payload.strategies || [];

  if ((!state.selectedStrategy || !selectedStrategy()) && state.strategies.length > 0) {
    state.selectedStrategy = state.strategies[0].strategy;
  }

  const strategyChanged = previousStrategy !== state.selectedStrategy;
  const nextSettled = selectedStrategy()?.settled_count ?? 0;

  if (strategyChanged) {
    resetPositionsView();
    resetOpenOrdersView();
    resetSettlementsPage();
    void refreshActiveTab(true);
    return;
  }

  if (state.selectedStrategy) {
    void refreshActiveTab(false);
  }

  if (previousSettled !== nextSettled && state.selectedStrategy && state.currentTab === "history") {
    resetSettlementsPage();
    void refreshActiveTab(true);
  }
}

function upsertStrategy(next) {
  const index = state.strategies.findIndex((item) => item.strategy === next.strategy);
  if (index === -1) {
    state.strategies.unshift(next);
  } else {
    state.strategies.splice(index, 1, next);
  }
  if (!state.selectedStrategy) state.selectedStrategy = next.strategy;
}

function resetSettlementsPage() {
  state.settlementsPage = {
    ...state.settlementsPage,
    strategy: state.selectedStrategy,
    range: HISTORY_RANGE,
    page: 1,
    total: 0,
    totalPages: 0,
    rows: [],
    loading: false,
    error: null,
  };
}

function resetPositionsView() {
  state.positionsView = {
    ...state.positionsView,
    strategy: state.selectedStrategy,
    rows: [],
    total: 0,
    loading: false,
    error: null,
  };
}

function resetOpenOrdersView() {
  state.openOrdersView = {
    ...state.openOrdersView,
    strategy: state.selectedStrategy,
    rows: [],
    total: 0,
    loading: false,
    error: null,
  };
}

async function loadPositions() {
  if (!state.selectedStrategy) {
    resetPositionsView();
    return;
  }

  const hasRows = state.positionsView.strategy === state.selectedStrategy && (state.positionsView.rows || []).length > 0;
  const requestId = state.positionsView.requestId + 1;
  state.positionsView = {
    ...state.positionsView,
    strategy: state.selectedStrategy,
    loading: !hasRows,
    error: null,
    requestId,
  };
  if (!hasRows) renderTable();

  const params = new URLSearchParams({ strategy: state.selectedStrategy });

  try {
    const payload = await fetchJson(`/api/positions?${params.toString()}`);
    if (state.positionsView.requestId !== requestId) return;

    state.positionsView = {
      ...state.positionsView,
      strategy: payload.strategy,
      rows: payload.rows || [],
      total: payload.total || 0,
      loading: false,
      error: null,
    };
  } catch (error) {
    if (state.positionsView.requestId !== requestId) return;

    state.positionsView = {
      ...state.positionsView,
      loading: false,
      error: `加载失败: ${error.message || error}`,
    };
  }

  renderTable();
}

async function loadOpenOrders() {
  if (!state.selectedStrategy) {
    resetOpenOrdersView();
    return;
  }

  const hasRows = state.openOrdersView.strategy === state.selectedStrategy && (state.openOrdersView.rows || []).length > 0;
  const requestId = state.openOrdersView.requestId + 1;
  state.openOrdersView = {
    ...state.openOrdersView,
    strategy: state.selectedStrategy,
    loading: !hasRows,
    error: null,
    requestId,
  };
  if (!hasRows) renderTable();

  const params = new URLSearchParams({ strategy: state.selectedStrategy });

  try {
    const payload = await fetchJson(`/api/open-orders?${params.toString()}`);
    if (state.openOrdersView.requestId !== requestId) return;

    state.openOrdersView = {
      ...state.openOrdersView,
      strategy: payload.strategy,
      rows: payload.rows || [],
      total: payload.total || 0,
      loading: false,
      error: null,
    };
  } catch (error) {
    if (state.openOrdersView.requestId !== requestId) return;

    state.openOrdersView = {
      ...state.openOrdersView,
      loading: false,
      error: `加载失败: ${error.message || error}`,
    };
  }

  renderTable();
}

async function loadClosedPositions() {
  if (!state.selectedStrategy) {
    resetSettlementsPage();
    return;
  }

  const hasRows = state.settlementsPage.strategy === state.selectedStrategy && (state.settlementsPage.rows || []).length > 0;
  const requestId = state.settlementsPage.requestId + 1;
  state.settlementsPage = {
    ...state.settlementsPage,
    strategy: state.selectedStrategy,
    range: HISTORY_RANGE,
    page: 1,
    loading: !hasRows,
    error: null,
    requestId,
  };
  if (!hasRows) renderTable();

  const params = new URLSearchParams({
    strategy: state.selectedStrategy,
    range: HISTORY_RANGE,
    page: "1",
    page_size: String(state.settlementsPage.pageSize),
  });

  try {
    const payload = await fetchJson(`/api/closed-positions?${params.toString()}`);
    if (state.settlementsPage.requestId !== requestId) return;

    state.settlementsPage = {
      ...state.settlementsPage,
      strategy: payload.strategy,
      range: payload.range || HISTORY_RANGE,
      page: payload.page,
      pageSize: payload.page_size,
      total: payload.total,
      totalPages: payload.total_pages,
      rows: payload.rows || [],
      loading: false,
      error: null,
    };
  } catch (error) {
    if (state.settlementsPage.requestId !== requestId) return;

    state.settlementsPage = {
      ...state.settlementsPage,
      loading: false,
      error: `加载失败: ${error.message || error}`,
    };
  }

  renderTable();
}

function render() {
  renderOverview();
  renderStrategies();
  renderTabs();
  renderTable();
}

function renderOverview() {
  const account = state.account;
  if (!account) {
    nodes.portfolioValue.textContent = "--";
    nodes.portfolioSecondary.textContent = "等待首个快照";
    nodes.portfolioSideValue.textContent = "--";
    nodes.statusStrip.innerHTML = "";
    nodes.heroActions.innerHTML = "";
    setAccountError("当前无账户快照", "tone-neutral");
    return;
  }

  nodes.portfolioValue.textContent = formatMoney(account.settled_pnl_usdc);
  nodes.portfolioSecondary.textContent = `累计已结算盈亏 · ${account.settled_count || 0} 笔`;
  nodes.portfolioSideValue.textContent = formatMoney(account.today_notional_usdc);

  nodes.statusStrip.innerHTML = [
    statusCard("运行状态", badge(statusLabel(account.runtime_status), account.runtime_status)),
    statusCard("Polymarket", badge(statusLabel(account.polymarket_ws_status), account.polymarket_ws_status)),
    statusCard("Binance", badge(statusLabel(account.binance_ws_status), account.binance_ws_status)),
  ].join("");

  nodes.heroActions.innerHTML = [
    actionCard("当前仓位", String(account.position_count ?? 0)),
    actionCard("未成交订单", String(account.open_order_count ?? 0)),
    actionCard("今日成交", String(account.today_trade_count ?? 0)),
  ].join("");

  setAccountError(account.last_error || "当前无错误", account.last_error ? "tone-negative" : "tone-neutral");
}

function renderStrategies() {
  nodes.strategyCount.textContent = `${state.strategies.length} 个策略`;
  nodes.strategySwitcher.innerHTML = state.strategies
    .map((strategy) => {
      const active = strategy.strategy === state.selectedStrategy ? "active" : "";
      return `
        <button class="strategy-chip ${active}" data-strategy="${escapeAttr(strategy.strategy)}" type="button">
          <div class="strategy-chip-head">
            <span class="strategy-chip-title">${escapeHtml(strategy.strategy)}</span>
            ${badge(statusLabel(strategy.status), strategy.status)}
          </div>
          <div class="strategy-chip-meta">
            <span>仓位 ${strategy.position_count ?? 0}</span>
            <span>${formatMoney(strategy.settled_pnl_usdc)}</span>
          </div>
        </button>
      `;
    })
    .join("");

}

function renderTabs() {
  const strategy = selectedStrategy();
  const positionsCount = strategy?.position_count ?? 0;
  const openOrdersCount = strategy?.open_order_count ?? 0;
  const historyCount = strategy?.settled_count ?? 0;

  const tabs = [
    { key: "positions", label: `持仓 ${positionsCount}` },
    { key: "orders", label: `未成交订单 ${openOrdersCount}` },
    { key: "history", label: `历史记录 ${historyCount}` },
  ];

  nodes.tableTabs.innerHTML = tabs
    .map(
      (tab) => `
        <button class="tab-chip ${tab.key === state.currentTab ? "active" : ""}" data-tab="${tab.key}" type="button">
          ${tab.label}
        </button>
      `,
    )
    .join("");

  nodes.searchInput.placeholder =
    state.currentTab === "history" ? "搜索历史市场或结果" : state.currentTab === "orders" ? "搜索状态" : "搜索市场或方向";
}

function renderTable() {
  const strategy = selectedStrategy();
  if (!strategy) {
    nodes.tableHead.innerHTML = "";
    nodes.tableBody.innerHTML = `<div class="empty-state">等待策略数据。</div>`;
    nodes.tableSummary.textContent = "暂无内容";
    return;
  }

  const view = currentTableView(strategy);
  nodes.tableHead.innerHTML = view.columns.map((column) => `<div>${column}</div>`).join("");
  nodes.tableBody.innerHTML = view.rowsHtml;
  nodes.tableSummary.textContent = view.summary || "";
}

function currentTableView(strategy) {
  if (state.currentTab === "orders") {
    return renderOrdersView(strategy);
  }
  if (state.currentTab === "history") {
    return renderHistoryView();
  }
  return renderPositionsView(strategy);
}

function renderPositionsView(strategy) {
  const columns = ["盘口", "方向", "持仓份额", "均价", "成本", "已实现盈亏"];
  const rows = filterRows(sortPositions(state.positionsView.rows || []));

  if (state.positionsView.loading && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">持仓加载中。</div>`,
      summary: "正在加载持仓",
    };
  }

  if (state.positionsView.error && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state tone-negative">${escapeHtml(state.positionsView.error)}</div>`,
      summary: "持仓加载失败",
    };
  }

  if (rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">当前没有匹配的持仓。</div>`,
      summary: `共 ${state.positionsView.total || 0} 条持仓`,
    };
  }

  return {
    columns,
    rowsHtml: rows.map(renderPositionRow).join(""),
    summary: `共 ${state.positionsView.total || rows.length} 条持仓`,
  };
}

function renderOrdersView(strategy) {
  const columns = ["盘口", "方向", "状态", "挂单价格", "剩余数量", "创建时间"];
  const rows = filterRows(sortOrders(state.openOrdersView.rows || []));

  if (state.openOrdersView.loading && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">未成交订单加载中。</div>`,
      summary: "正在加载未成交订单",
    };
  }

  if (state.openOrdersView.error && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state tone-negative">${escapeHtml(state.openOrdersView.error)}</div>`,
      summary: "未成交订单加载失败",
    };
  }

  if (rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">当前没有匹配的未成交订单。</div>`,
      summary: `未成交订单 ${state.openOrdersView.total || 0} 条`,
    };
  }

  return {
    columns,
    rowsHtml: rows.map(renderOrderRow).join(""),
    summary: `未成交订单 ${state.openOrdersView.total || rows.length} 条`,
  };
}

function renderHistoryView() {
  const columns = ["盘口", "结果", "买入均价", "买入份额", "已结算盈亏", "结算时间"];
  const rows = filterRows(sortHistory(state.settlementsPage.rows || []));

  if (state.settlementsPage.loading && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">历史记录加载中。</div>`,
      summary: "",
    };
  }

  if (state.settlementsPage.error && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state tone-negative">${escapeHtml(state.settlementsPage.error)}</div>`,
      summary: "",
    };
  }

  if (rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">当前没有匹配的历史记录。</div>`,
      summary: "",
    };
  }

  return {
    columns,
    rowsHtml: rows.map(renderHistoryRow).join(""),
    summary: "",
  };
}

function renderPositionRow(row) {
  return `
    <div class="table-row">
      ${renderMarketCell(
        row.market_slug || row.asset_id,
        `${formatShare(row.size)} 份`,
        marketIconKind(row.market_slug),
        toneKey(row.outcome),
        "",
        buildEventUrl(row.market_slug),
      )}
      <div class="cell-value">
        <span class="outcome-chip ${toneKey(row.outcome)}">${escapeHtml(sideLabel(row.outcome || "unknown"))}</span>
      </div>
      <div class="cell-value">${formatShare(row.size)}<span class="cell-sub">份额</span></div>
      <div class="cell-value">${formatQuote(row.avg_price)}</div>
      <div class="cell-value">${formatMoney(row.open_cost)}</div>
      <div class="cell-value ${toneClass(row.realized_pnl)}">${formatMoney(row.realized_pnl)}</div>
    </div>
  `;
}

function renderHistoryRow(row) {
  const entryOutcome = pickOutcome(row.outcome, row.oppositeOutcome);
  const settlementOutcome = historySettlementResult(row);
  return `
    <div class="table-row">
      ${renderMarketCell(
        row.slug,
        formatTime(normalizeEpochMs(row.timestamp), true),
        marketIconKind(row.slug),
        toneKey(entryOutcome),
        row.icon,
        buildEventUrl(row.slug),
      )}
      <div class="cell-value">
        <span class="outcome-chip ${toneKey(settlementOutcome)}">${escapeHtml(sideLabel(settlementOutcome))}</span>
      </div>
      <div class="cell-value">${formatQuote(row.avgPrice)}</div>
      <div class="cell-value">${formatShare(row.totalBought)}<span class="cell-sub">买入份额</span></div>
      <div class="cell-value ${toneClass(row.realizedPnl)}">${formatMoney(row.realizedPnl)}</div>
      <div class="cell-value">${formatTime(normalizeEpochMs(row.timestamp), true)}</div>
    </div>
  `;
}

function renderOrderRow(row) {
  return `
    <div class="table-row">
      ${renderMarketCell(
        row.market_slug || row.order_id,
        formatTime(row.created_at_ms, true),
        marketIconKind(row.market_slug),
        toneKey(row.side),
        "",
        buildEventUrl(row.market_slug),
      )}
      <div class="cell-value">
        <span class="outcome-chip ${toneKey(row.side)}">${escapeHtml(sideLabel(row.side || "unknown"))}</span>
      </div>
      <div class="cell-value">${badge(statusLabel(row.status), row.status)}</div>
      <div class="cell-value">${formatQuote(row.price)}</div>
      <div class="cell-value">${formatShare(row.size)}<span class="cell-sub">剩余份额</span></div>
      <div class="cell-value">${formatTime(row.created_at_ms, true)}</div>
    </div>
  `;
}

function renderMarketCell(title, note, iconKind, outcomeKind, iconUrl = "", href = "") {
  const titleHtml = href
    ? `<a class="market-link" href="${escapeAttr(href)}" target="_blank" rel="noreferrer noopener">${escapeHtml(title)}</a>`
    : escapeHtml(title);
  return `
    <div class="market-cell">
      ${renderMarketIcon(iconKind, iconUrl)}
      <div class="market-copy">
        <p class="market-title">${titleHtml}</p>
        <div class="market-subline">
          <span class="outcome-chip ${outcomeKind}">${outcomeText(outcomeKind)}</span>
          <span class="muted-note">${escapeHtml(note)}</span>
        </div>
      </div>
    </div>
  `;
}

function renderMarketIcon(iconKind, iconUrl) {
  const safeUrl = String(iconUrl || "").trim();
  if (safeUrl) {
    return `
      <div class="market-icon market-icon-image">
        <img src="${escapeAttr(safeUrl)}" alt="" loading="lazy" />
      </div>
    `;
  }

  return `<div class="market-icon ${iconKind}">${marketIconLabel(iconKind)}</div>`;
}

function buildEventUrl(slug) {
  const safeSlug = String(slug || "").trim();
  if (!safeSlug) return "";
  return `https://polymarket.com/zh/event/${encodeURIComponent(safeSlug)}`;
}

// 历史记录里的“结果”列展示最终结算结果，不展示当时买入方向。
function historySettlementResult(row) {
  const curPrice = optionalNumeric(row.curPrice);

  if (curPrice !== null && curPrice >= 0.999) {
    return pickOutcome(row.outcome, row.oppositeOutcome);
  }
  if (curPrice !== null && curPrice <= 0.001) {
    return pickOutcome(row.oppositeOutcome, row.outcome);
  }

  const realizedPnl = optionalNumeric(row.realizedPnl);
  if (realizedPnl !== null && realizedPnl < 0) {
    return pickOutcome(row.oppositeOutcome, row.outcome);
  }
  if (realizedPnl !== null && realizedPnl > 0) {
    return pickOutcome(row.outcome, row.oppositeOutcome);
  }

  return pickOutcome(row.outcome, row.oppositeOutcome);
}

function bindEvents() {
  nodes.strategySwitcher.addEventListener("click", (event) => {
    const button = event.target.closest("[data-strategy]");
    if (!button || button.dataset.strategy === state.selectedStrategy) return;
    state.selectedStrategy = button.dataset.strategy;
    resetPositionsView();
    resetOpenOrdersView();
    resetSettlementsPage();
    void refreshActiveTab(true);
    render();
  });

  nodes.tableTabs.addEventListener("click", (event) => {
    const button = event.target.closest("[data-tab]");
    if (!button || button.dataset.tab === state.currentTab) return;
    state.currentTab = button.dataset.tab;
    void refreshActiveTab(true);
    renderTabs();
    renderTable();
  });

  nodes.searchInput.addEventListener("input", (event) => {
    state.search = event.target.value.trim().toLowerCase();
    renderTable();
  });
}

async function refreshActiveTab(forceReload = false) {
  if (!state.selectedStrategy) return;

  if (state.currentTab === "history") {
    if (
      forceReload ||
      state.settlementsPage.strategy !== state.selectedStrategy ||
      state.settlementsPage.range !== HISTORY_RANGE
    ) {
      resetSettlementsPage();
    }
    await loadClosedPositions();
    return;
  }

  if (state.currentTab === "orders") {
    if (forceReload || state.openOrdersView.strategy !== state.selectedStrategy) {
      resetOpenOrdersView();
    }
    await loadOpenOrders();
    return;
  }

  if (forceReload || state.positionsView.strategy !== state.selectedStrategy) {
    resetPositionsView();
  }
  await loadPositions();
}

function filterRows(rows) {
  if (!state.search) return rows;
  return rows.filter((row) => {
    const historyResult = row.timestamp ? historySettlementResult(row) : "";
    const haystack = [
      row.market_slug,
      row.slug,
      row.title,
      row.outcome,
      row.oppositeOutcome,
      ...outcomeSearchTerms(row.outcome, row.oppositeOutcome, historyResult),
      row.asset_id,
      row.asset,
      row.strategy,
      row.status,
      row.order_id,
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
    return haystack.includes(state.search);
  });
}

function sortPositions(rows) {
  return [...rows].sort((left, right) => {
    const byTrade = (right.last_trade_ms || 0) - (left.last_trade_ms || 0);
    if (byTrade !== 0) return byTrade;
    return numeric(right.open_cost) - numeric(left.open_cost);
  });
}

function sortOrders(rows) {
  return [...rows].sort((left, right) => (right.created_at_ms || 0) - (left.created_at_ms || 0));
}

function sortHistory(rows) {
  return [...rows].sort((left, right) => (right.timestamp || 0) - (left.timestamp || 0));
}

function statusCard(label, value) {
  return `
    <div class="status-card">
      <span class="status-card-label">${label}</span>
      <div class="status-card-value">${value}</div>
    </div>
  `;
}

function actionCard(label, value) {
  return `
    <div class="action-card">
      <span class="action-card-label">${label}</span>
      <div class="action-card-value">${value}</div>
    </div>
  `;
}

function badge(label, status) {
  const safe = (status || "unknown").toLowerCase();
  return `<span class="status-pill status-${safe}">${escapeHtml(label)}</span>`;
}

function marketIconKind(value) {
  const text = (value || "").toLowerCase();
  if (text.includes("ethereum") || text.includes("eth")) return "eth";
  if (text.includes("bitcoin") || text.includes("btc")) return "btc";
  return "default";
}

function marketIconLabel(kind) {
  if (kind === "eth") return "E";
  if (kind === "btc") return "B";
  return "M";
}

function toneKey(value) {
  return normalizedOutcome(value) || "neutral";
}

function sideLabel(side) {
  const outcome = normalizedOutcome(side);
  if (outcome === "yes") return "Up";
  if (outcome === "no") return "Down";
  return side || "未知";
}

function outcomeText(value) {
  const outcome = normalizedOutcome(value);
  if (outcome === "yes") return "看涨";
  if (outcome === "no") return "看跌";
  return "市场";
}

function normalizedOutcome(value) {
  const text = outcomeValue(value).toLowerCase();
  if (text === "up" || text === "yes") return "yes";
  if (text === "down" || text === "no") return "no";
  return "";
}

function outcomeValue(value) {
  return String(value || "").trim();
}

function pickOutcome(outcome, oppositeOutcome) {
  return outcomeValue(outcome) || outcomeValue(oppositeOutcome) || "unknown";
}

function optionalNumeric(value) {
  const text = outcomeValue(value);
  return text ? numeric(text) : null;
}

function outcomeSearchTerms(...values) {
  return values.flatMap((value) => {
    const text = outcomeValue(value);
    if (!text) return [];
    return [sideLabel(text), outcomeText(text)];
  });
}

function statusLabel(status) {
  const safe = (status || "").toLowerCase();
  const map = {
    running: "运行中",
    degraded: "降级",
    starting: "启动中",
    connecting: "连接中",
    connected: "已连接",
    disconnected: "已断开",
    reconnecting: "重连中",
    live: "实时",
    error: "错误",
    stopped: "已停止",
    idle: "空闲",
    submitted: "已提交",
    unknown: "未知",
  };
  return map[safe] || status || "未知";
}

function formatMoney(value) {
  const numericValue = numeric(value);
  const sign = numericValue > 0 ? "" : "";
  return `${sign}$${numericValue.toFixed(2)}`;
}

function formatQuote(value) {
  const amount = numeric(value);
  if (amount >= 0 && amount <= 1) {
    return `${Math.round(amount * 100)}¢`;
  }
  return `$${amount.toFixed(2)}`;
}

function formatShare(value) {
  return numeric(value).toFixed(1);
}

function formatTime(value, compact = false) {
  if (!value) return "暂无";
  const options = compact
    ? { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }
    : { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit" };
  return new Date(value).toLocaleString("zh-CN", options);
}

function normalizeEpochMs(value) {
  const raw = numeric(value);
  if (Math.abs(raw) < 1_000_000_000_000) {
    return raw * 1000;
  }
  return raw;
}

function toneClass(value) {
  const amount = numeric(value);
  if (amount > 0) return "tone-positive";
  if (amount < 0) return "tone-negative";
  return "tone-neutral";
}

function numeric(value) {
  const parsed = Number(value || 0);
  return Number.isFinite(parsed) ? parsed : 0;
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function escapeAttr(value) {
  return escapeHtml(value);
}

bindEvents();
setConnectionStatus("轮询启动中", "degraded");
void loadSnapshot();
startSnapshotPolling();
