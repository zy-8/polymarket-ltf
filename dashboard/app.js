const state = {
  account: null,
  strategies: [],
  selectedStrategy: null,
  currentTab: "positions",
  search: "",
  infoPollTimer: null,
  positionsView: {
    strategy: null,
    rows: [],
    visibleCount: 10,
    total: 0,
    loading: false,
    error: null,
    requestId: 0,
  },
  openOrdersView: {
    strategy: null,
    rows: [],
    visibleCount: 10,
    total: 0,
    loading: false,
    error: null,
    requestId: 0,
  },
  settlementsPage: {
    strategy: null,
    range: "all",
    page: 1,
    pageSize: 10,
    total: 0,
    totalPages: 0,
    rows: [],
    loading: false,
    error: null,
    requestId: 0,
  },
};

const HISTORY_RANGE = "all";
const PAGE_SIZE = 10;
const SCROLL_THRESHOLD_PX = 24;

const nodes = {
  connectionStatus: document.getElementById("connection-status"),
  portfolioValue: document.getElementById("portfolio-value"),
  portfolioSecondary: document.getElementById("portfolio-secondary"),
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

async function loadInfo() {
  try {
    applyInfo(await fetchJson("/api/info"));
    setConnectionStatus("轮询同步", "connected");
    render();
  } catch (error) {
    setConnectionStatus("轮询失败", "disconnected");
    setAccountError(`info 加载失败: ${error.message || error}`, "tone-negative");
    render();
  }
}

function startInfoPolling() {
  if (state.infoPollTimer) return;
  state.infoPollTimer = window.setInterval(() => {
    void loadInfo();
  }, 3000);
}

function stopInfoPolling() {
  if (!state.infoPollTimer) return;
  window.clearInterval(state.infoPollTimer);
  state.infoPollTimer = null;
}

function applyInfo(payload) {
  const previousStrategy = state.selectedStrategy;
  const previousClosed = selectedStrategy()?.closed_count ?? 0;

  state.account = payload.account;
  state.strategies = payload.strategies || [];

  if ((!state.selectedStrategy || !selectedStrategy()) && state.strategies.length > 0) {
    state.selectedStrategy = state.strategies[0].strategy;
  }

  const strategyChanged = previousStrategy !== state.selectedStrategy;
  const nextClosed = selectedStrategy()?.closed_count ?? 0;

  if (strategyChanged) {
    resetPositionsView();
    resetOpenOrdersView();
    resetSettlementsPage();
    void refreshActiveTab(true);
    return;
  }

  if (previousClosed !== nextClosed && state.selectedStrategy && state.currentTab === "history") {
    resetSettlementsPage();
    void refreshActiveTab(true);
    return;
  }

  if (state.selectedStrategy) {
    void refreshActiveTab(false);
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
    pageSize: PAGE_SIZE,
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
    visibleCount: PAGE_SIZE,
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
    visibleCount: PAGE_SIZE,
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

async function loadPositionsPage(options = {}) {
  if (!state.selectedStrategy) {
    resetSettlementsPage();
    return;
  }

  const { append = false } = options;
  const previousScrollTop = append ? nodes.tableBody.scrollTop : 0;
  const previousScrollHeight = append ? nodes.tableBody.scrollHeight : 0;
  const nextPage = append ? state.settlementsPage.page + 1 : 1;
  const hasRows = state.settlementsPage.strategy === state.selectedStrategy && (state.settlementsPage.rows || []).length > 0;
  const requestId = state.settlementsPage.requestId + 1;
  state.settlementsPage = {
    ...state.settlementsPage,
    strategy: state.selectedStrategy,
    range: HISTORY_RANGE,
    page: append ? state.settlementsPage.page : 1,
    loading: append || !hasRows,
    error: null,
    requestId,
  };
  if (!hasRows) renderTable();

  const params = new URLSearchParams({
    strategy: state.selectedStrategy,
    range: HISTORY_RANGE,
    page: String(nextPage),
    page_size: String(state.settlementsPage.pageSize),
  });

  try {
    const payload = await fetchJson(`/api/positions-page?${params.toString()}`);
    if (state.settlementsPage.requestId !== requestId) return;

    state.settlementsPage = {
      ...state.settlementsPage,
      strategy: payload.strategy,
      range: payload.range || HISTORY_RANGE,
      page: payload.page,
      pageSize: payload.page_size,
      total: payload.total,
      totalPages: payload.total_pages,
      rows: append ? [...(state.settlementsPage.rows || []), ...(payload.rows || [])] : payload.rows || [],
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

  if (append) {
    const nextScrollHeight = nodes.tableBody.scrollHeight;
    const heightDelta = Math.max(0, nextScrollHeight - previousScrollHeight);
    nodes.tableBody.scrollTop = previousScrollTop + heightDelta;
  }
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
    nodes.portfolioSecondary.textContent = "等待首个 info";
    nodes.statusStrip.innerHTML = "";
    nodes.heroActions.innerHTML = "";
    setAccountError("当前无账户 info", "tone-neutral");
    return;
  }

  nodes.portfolioValue.textContent = formatMoney(account.settled_pnl_usdc);
  nodes.portfolioSecondary.textContent = `累计已结算盈亏 · ${account.closed_count || 0} 笔`;

  nodes.statusStrip.innerHTML = [
    statusCard("运行状态", badge(statusLabel(account.runtime_status), account.runtime_status)),
    statusCard("Polymarket", badge(statusLabel(account.polymarket_ws_status), account.polymarket_ws_status)),
    statusCard("Binance", badge(statusLabel(account.binance_ws_status), account.binance_ws_status)),
  ].join("");

  nodes.heroActions.innerHTML = [
    actionCard("成交总数", `${account.closed_count || 0} 笔`),
    actionCard("成交胜率", formatRate(account.closed_win_count, account.closed_count)),
    actionCard("成交胜次数", `${account.closed_win_count || 0} 次`),
    actionCard("成交负次数", `${account.closed_loss_count || 0} 次`),
    actionCard("未成交总数", `${account.missed_count || 0} 笔`),
    actionCard("未成交胜率", formatRate(account.missed_win_count, account.missed_count)),
    actionCard("未成交胜次数", `${account.missed_win_count || 0} 次`),
    actionCard("未成交负次数", `${account.missed_loss_count || 0} 次`),
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
            <span>触发总数 ${strategy.trigger_count ?? 0}</span>
            <span>成交总数 ${strategy.closed_count ?? 0}</span>
            <span>成交胜率 ${formatRate(strategy.closed_win_count, strategy.closed_count)}</span>
            <span>成交胜次数 ${strategy.closed_win_count ?? 0}</span>
            <span>成交负次数 ${strategy.closed_loss_count ?? 0}</span>
            <span>未成交总数 ${strategy.missed_count ?? 0}</span>
            <span>未成交胜率 ${formatRate(strategy.missed_win_count, strategy.missed_count)}</span>
            <span>未成交胜次数 ${strategy.missed_win_count ?? 0}</span>
            <span>未成交负次数 ${strategy.missed_loss_count ?? 0}</span>
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
  const historyCount = strategy?.closed_count ?? 0;

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
    nodes.tableBody.classList.add("is-empty");
    nodes.tableSummary.textContent = "暂无内容";
    return;
  }

  const view = currentTableView(strategy);
  nodes.tableHead.innerHTML = view.columns.map((column) => `<div>${column}</div>`).join("");
  nodes.tableBody.innerHTML = view.rowsHtml;
  nodes.tableBody.classList.toggle("is-empty", Boolean(view.isEmpty));
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
  const visibleRows = rows.slice(0, state.positionsView.visibleCount || PAGE_SIZE);

  if (state.positionsView.loading && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">持仓加载中。</div>`,
      summary: "正在加载持仓",
      isEmpty: true,
    };
  }

  if (state.positionsView.error && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state tone-negative">${escapeHtml(state.positionsView.error)}</div>`,
      summary: "持仓加载失败",
      isEmpty: true,
    };
  }

  if (rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">当前没有匹配的持仓。</div>`,
      summary: `共 ${state.positionsView.total || 0} 条持仓`,
      isEmpty: true,
    };
  }

  return {
    columns,
    rowsHtml: `${visibleRows.map(renderPositionRow).join("")}${renderLoadMoreMarker(visibleRows.length < rows.length)}`,
    summary: `显示 ${visibleRows.length}/${rows.length} 条持仓`,
    isEmpty: false,
  };
}

function renderOrdersView(strategy) {
  const columns = ["盘口", "方向", "状态", "挂单价格", "剩余数量", "创建时间"];
  const rows = filterRows(sortOrders(state.openOrdersView.rows || []));
  const visibleRows = rows.slice(0, state.openOrdersView.visibleCount || PAGE_SIZE);

  if (state.openOrdersView.loading && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">未成交订单加载中。</div>`,
      summary: "正在加载未成交订单",
      isEmpty: true,
    };
  }

  if (state.openOrdersView.error && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state tone-negative">${escapeHtml(state.openOrdersView.error)}</div>`,
      summary: "未成交订单加载失败",
      isEmpty: true,
    };
  }

  if (rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">当前没有匹配的未成交订单。</div>`,
      summary: `未成交订单 ${state.openOrdersView.total || 0} 条`,
      isEmpty: true,
    };
  }

  return {
    columns,
    rowsHtml: `${visibleRows.map(renderOrderRow).join("")}${renderLoadMoreMarker(visibleRows.length < rows.length)}`,
    summary: `显示 ${visibleRows.length}/${rows.length} 条未成交订单`,
    isEmpty: false,
  };
}

function renderHistoryView() {
  const columns = ["市场", "方向", "均价", "份额", "结算盈亏", "抓取时间"];
  const rows = filterRows(sortHistory(state.settlementsPage.rows || []));

  if (state.settlementsPage.loading && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">历史记录加载中。</div>`,
      summary: "历史记录加载中",
      isEmpty: true,
    };
  }

  if (state.settlementsPage.error && rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state tone-negative">${escapeHtml(state.settlementsPage.error)}</div>`,
      summary: "历史记录加载失败",
      isEmpty: true,
    };
  }

  if (rows.length === 0) {
    return {
      columns,
      rowsHtml: `<div class="empty-state">当前没有匹配的历史记录。</div>`,
      summary: "暂无历史记录",
      isEmpty: true,
    };
  }

  const hasMore = state.settlementsPage.page < state.settlementsPage.totalPages;
  return {
    columns,
    rowsHtml: `${rows.map(renderHistoryRow).join("")}${renderLoadMoreMarker(hasMore || state.settlementsPage.loading)}`,
    summary: `显示 ${rows.length}/${state.settlementsPage.total || rows.length} 条历史记录 · 第 ${state.settlementsPage.page}/${Math.max(state.settlementsPage.totalPages || 1, 1)} 页`,
    isEmpty: false,
  };
}

function renderLoadMoreMarker(visible) {
  if (!visible) return "";
  return `<div class="table-load-more">继续向下滚动加载更多</div>`;
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
  const marketSlug = row.marketSlug || row.market_slug || row.asset;
  const settlementOutcome = historySettlementResult(row);
  const pnl = row.cashPnl ?? row.cash_pnl ?? row.realizedPnl ?? row.realized_pnl ?? "0";
  const size = row.totalBought ?? row.total_bought ?? row.size ?? "0";
  return `
    <div class="table-row">
      ${renderMarketCell(
        marketSlug,
        formatTime(normalizeEpochMs(row.timestamp), true),
        marketIconKind(marketSlug),
        toneKey(settlementOutcome),
        "",
        buildEventUrl(marketSlug),
      )}
      <div class="cell-value">
        <span class="outcome-chip ${toneKey(settlementOutcome)}">${escapeHtml(sideLabel(settlementOutcome))}</span>
      </div>
      <div class="cell-value">${formatQuote(row.avgPrice ?? row.avg_price)}</div>
      <div class="cell-value">${formatShare(size)}<span class="cell-sub">份额</span></div>
      <div class="cell-value ${toneClass(pnl)}">${formatMoney(pnl)}</div>
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
  const outcome = row.outcome;
  const opposite = oppositeOutcome(outcome);
  const curPrice = optionalNumeric(row.curPrice ?? row.cur_price);

  if (curPrice !== null && curPrice >= 0.999) {
    return pickOutcome(outcome, opposite);
  }
  if (curPrice !== null && curPrice <= 0.001) {
    return pickOutcome(opposite, outcome);
  }

  const realizedPnl = optionalNumeric(row.cashPnl ?? row.cash_pnl ?? row.realizedPnl ?? row.realized_pnl);
  if (realizedPnl !== null && realizedPnl < 0) {
    return pickOutcome(opposite, outcome);
  }
  if (realizedPnl !== null && realizedPnl > 0) {
    return pickOutcome(outcome, opposite);
  }

  return pickOutcome(outcome, opposite);
}

function bindEvents() {
  nodes.strategySwitcher.addEventListener("click", (event) => {
    const button = event.target.closest("[data-strategy]");
    if (!button || button.dataset.strategy === state.selectedStrategy) return;
    state.selectedStrategy = button.dataset.strategy;
    resetPositionsView();
    resetOpenOrdersView();
    resetSettlementsPage();
    scrollTableToTop();
    void refreshActiveTab(true);
    render();
  });

  nodes.tableTabs.addEventListener("click", (event) => {
    const button = event.target.closest("[data-tab]");
    if (!button || button.dataset.tab === state.currentTab) return;
    state.currentTab = button.dataset.tab;
    scrollTableToTop();
    void refreshActiveTab(true);
    renderTabs();
    renderTable();
  });

  nodes.searchInput.addEventListener("input", (event) => {
    state.search = event.target.value.trim().toLowerCase();
    syncVisibleCountWithSearch();
    scrollTableToTop();
    renderTable();
  });

  nodes.tableBody.addEventListener("scroll", () => {
    if (!shouldLoadMoreOnScroll(nodes.tableBody)) return;
    void loadMoreRows();
  });
}

async function refreshActiveTab(forceReload = false) {
  if (!state.selectedStrategy) return;

  if (state.currentTab === "history") {
    const strategyChanged = state.settlementsPage.strategy !== state.selectedStrategy;
    const rangeChanged = state.settlementsPage.range !== HISTORY_RANGE;
    const hasRows = (state.settlementsPage.rows || []).length > 0;

    if (forceReload || strategyChanged || rangeChanged) {
      resetSettlementsPage();
    }
    if (!forceReload && !strategyChanged && !rangeChanged && hasRows) {
      return;
    }
    await loadPositionsPage();
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
      row.marketSlug,
      row.outcome,
      ...outcomeSearchTerms(row.outcome, historyResult),
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

function syncVisibleCountWithSearch() {
  if (state.currentTab === "orders") {
    state.openOrdersView.visibleCount = PAGE_SIZE;
    return;
  }
  if (state.currentTab === "positions") {
    state.positionsView.visibleCount = PAGE_SIZE;
  }
}

function shouldLoadMoreOnScroll(container) {
  return container.scrollTop + container.clientHeight >= container.scrollHeight - SCROLL_THRESHOLD_PX;
}

async function loadMoreRows() {
  if (state.currentTab === "history") {
    if (state.settlementsPage.loading) return;
    if (state.settlementsPage.page >= state.settlementsPage.totalPages) return;
    await loadPositionsPage({ append: true });
    return;
  }

  if (state.currentTab === "orders") {
    const rows = filterRows(sortOrders(state.openOrdersView.rows || []));
    if (state.openOrdersView.loading) return;
    if ((state.openOrdersView.visibleCount || PAGE_SIZE) >= rows.length) return;
    state.openOrdersView.visibleCount += PAGE_SIZE;
    renderTable();
    return;
  }

  const rows = filterRows(sortPositions(state.positionsView.rows || []));
  if (state.positionsView.loading) return;
  if ((state.positionsView.visibleCount || PAGE_SIZE) >= rows.length) return;
  state.positionsView.visibleCount += PAGE_SIZE;
  renderTable();
}

function scrollTableToTop() {
  nodes.tableBody.scrollTop = 0;
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

function oppositeOutcome(outcome) {
  const normalized = normalizedOutcome(outcome);
  if (normalized === "yes") return "No";
  if (normalized === "no") return "Yes";
  return "";
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

function formatRate(winCount, totalCount) {
  const total = numeric(totalCount);
  if (total <= 0) return "--";
  return `${((numeric(winCount) / total) * 100).toFixed(1)}%`;
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
void loadInfo();
startInfoPolling();
