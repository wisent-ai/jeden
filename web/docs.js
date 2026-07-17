const pages = [...document.querySelectorAll('[data-doc]')];
const sidebar = document.querySelector('[data-sidebar]');
const menuButton = document.querySelector('[data-docs-menu]');
const searchOverlay = document.querySelector('[data-search-overlay]');
const searchInput = document.querySelector('[data-search-input]');
const searchResults = document.querySelector('[data-search-results]');
const toc = document.querySelector('[data-toc]');

const slugFromPath = () => {
  const parts = location.pathname.replace(/\/+$/, '').split('/').filter(Boolean);
  return parts[0] === 'docs' && parts[1] ? parts[1] : 'overview';
};

const pageForSlug = (slug) => pages.find((page) => page.dataset.doc === slug) || pages[0];

const buildToc = (page) => {
  const headings = [...page.querySelectorAll('h2')];
  toc.innerHTML = '<span>On this page</span>';
  headings.forEach((heading, index) => {
    if (!heading.id) heading.id = `${page.dataset.doc}-${index + 1}`;
    const link = document.createElement('a');
    link.href = `#${heading.id}`;
    link.textContent = heading.textContent;
    toc.appendChild(link);
  });
};

const activate = (slug) => {
  const page = pageForSlug(slug);
  pages.forEach((candidate) => candidate.classList.toggle('active', candidate === page));
  document.querySelectorAll('[data-route]').forEach((link) => {
    link.classList.toggle('active', link.dataset.route === page.dataset.doc);
  });
  document.title = `${page.dataset.title} — Jeden docs`;
  document.querySelector('meta[name="description"]')?.setAttribute('content', page.dataset.description || 'Jeden documentation');
  buildToc(page);
  sidebar?.classList.remove('open');
  menuButton?.setAttribute('aria-expanded', 'false');
};

activate(slugFromPath());

menuButton?.addEventListener('click', () => {
  const open = !sidebar.classList.contains('open');
  sidebar.classList.toggle('open', open);
  menuButton.setAttribute('aria-expanded', String(open));
});

document.addEventListener('click', (event) => {
  const route = event.target.closest('a[data-route]');
  if (!route) return;
  const url = new URL(route.href);
  if (url.origin !== location.origin) return;
  event.preventDefault();
  history.pushState({}, '', url.pathname);
  activate(route.dataset.route);
  window.scrollTo(0, 0);
});
window.addEventListener('popstate', () => activate(slugFromPath()));

const openSearch = () => {
  searchOverlay.classList.add('open');
  searchOverlay.setAttribute('aria-hidden', 'false');
  searchInput.value = '';
  renderSearch('');
  setTimeout(() => searchInput.focus(), 20);
};
const closeSearch = () => {
  searchOverlay.classList.remove('open');
  searchOverlay.setAttribute('aria-hidden', 'true');
};

const searchIndex = pages.map((page) => ({
  slug: page.dataset.doc,
  title: page.dataset.title,
  description: page.dataset.description,
  text: page.textContent.replace(/\s+/g, ' ').trim().toLowerCase()
}));

function renderSearch(query) {
  const normalized = query.trim().toLowerCase();
  const terms = normalized.split(/\s+/).filter(Boolean);
  const matches = terms.length
    ? searchIndex.filter((item) => {
        const haystack = `${item.title} ${item.description} ${item.text}`.toLowerCase();
        return terms.every((term) => haystack.includes(term));
      })
    : searchIndex.slice(0, 7);
  searchResults.innerHTML = '';
  if (!matches.length) {
    searchResults.innerHTML = '<div class="search-empty">No documentation matched that search.</div>';
    return;
  }
  matches.forEach((item) => {
    const link = document.createElement('a');
    link.className = 'search-result';
    link.href = item.slug === 'overview' ? '/docs/' : `/docs/${item.slug}/`;
    link.dataset.route = item.slug;
    link.innerHTML = `<b>${item.title}</b><span>${item.description}</span>`;
    link.addEventListener('click', () => closeSearch());
    searchResults.appendChild(link);
  });
}

document.querySelector('[data-search-open]')?.addEventListener('click', openSearch);
document.querySelector('[data-search-close]')?.addEventListener('click', closeSearch);
searchOverlay?.addEventListener('click', (event) => { if (event.target === searchOverlay) closeSearch(); });
searchInput?.addEventListener('input', () => renderSearch(searchInput.value));
document.addEventListener('keydown', (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
    event.preventDefault();
    searchOverlay.classList.contains('open') ? closeSearch() : openSearch();
  }
  if (event.key === 'Escape') closeSearch();
});

document.querySelectorAll('.code-block').forEach((block) => {
  const button = document.createElement('button');
  button.className = 'copy-code';
  button.type = 'button';
  button.textContent = 'COPY';
  button.addEventListener('click', async () => {
    const code = block.querySelector('code')?.innerText || '';
    await navigator.clipboard.writeText(code.replace(/^\$ /gm, ''));
    button.textContent = 'COPIED';
    setTimeout(() => { button.textContent = 'COPY'; }, 1200);
  });
  block.appendChild(button);
});

document.querySelectorAll('[data-year]').forEach((node) => { node.textContent = new Date().getFullYear(); });
