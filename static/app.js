(() => {
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const anime = window.anime;
  if (!reduce && anime) {
    anime.animate('[data-motion]', {
      opacity: [0, 1],
      y: [18, 0],
      delay: anime.stagger(70),
      duration: 520,
      ease: 'out(3)'
    });
  } else {
    document.querySelectorAll('[data-motion]').forEach((el) => {
      el.style.opacity = '1';
      el.style.transform = 'none';
    });
  }

  document.querySelectorAll('[data-confirm]').forEach((form) => {
    form.addEventListener('submit', (event) => {
      if (!confirm(form.dataset.confirm)) event.preventDefault();
    });
  });

  document.querySelectorAll('time[data-unix]').forEach((el) => {
    const value = Number(el.dataset.unix) * 1000;
    if (Number.isFinite(value)) el.textContent = new Date(value).toLocaleString();
  });
})();
