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

  const confirmForms = document.querySelectorAll('[data-confirm]');
  if (confirmForms.length) {
    const dialog = document.createElement('dialog');
    dialog.className = 'confirm-modal';
    dialog.setAttribute('aria-labelledby', 'confirm-title');
    dialog.setAttribute('aria-describedby', 'confirm-message');
    dialog.innerHTML = '<div class="confirm-card"><span class="confirm-mark" aria-hidden="true">!</span><p class="eyebrow">Confirm action</p><h2 id="confirm-title">Are you sure?</h2><p id="confirm-message"></p><div class="confirm-actions"><button type="button" class="secondary" data-confirm-cancel>Cancel</button><button type="button" class="danger" data-confirm-submit>Confirm</button></div></div>';
    document.body.append(dialog);

    const card = dialog.querySelector('.confirm-card');
    const message = dialog.querySelector('#confirm-message');
    const cancel = dialog.querySelector('[data-confirm-cancel]');
    const submit = dialog.querySelector('[data-confirm-submit]');
    let pendingForm;

    const close = () => {
      if (!dialog.open) return;
      if (!reduce && anime) {
        anime.animate(card, {
          opacity: [1, 0],
          scale: [1, 0.94],
          y: [0, 16],
          duration: 180,
          ease: 'in(2)',
          onComplete: () => dialog.close()
        });
      } else {
        dialog.close();
      }
    };

    confirmForms.forEach((form) => {
      form.addEventListener('submit', (event) => {
        event.preventDefault();
        pendingForm = form;
        message.textContent = form.dataset.confirm;
        dialog.showModal();
        if (!reduce && anime) {
          anime.animate(card, {
            opacity: [0, 1],
            scale: [0.9, 1],
            y: [24, 0],
            duration: 360,
            ease: 'out(4)'
          });
        }
        cancel.focus();
      });
    });

    cancel.addEventListener('click', close);
    dialog.addEventListener('cancel', (event) => {
      event.preventDefault();
      close();
    });
    dialog.addEventListener('click', (event) => {
      if (event.target === dialog) close();
    });
    submit.addEventListener('click', () => {
      if (!pendingForm) return;
      submit.disabled = true;
      pendingForm.submit();
    });
    dialog.addEventListener('close', () => {
      pendingForm = undefined;
      submit.disabled = false;
    });
  }

  document.querySelectorAll('[data-print]').forEach((button) => {
    button.addEventListener('click', () => window.print());
  });

  const search = document.querySelector('[data-template-search]');
  if (search) {
    const cards = [...document.querySelectorAll('[data-template-card]')];
    const empty = document.querySelector('[data-template-empty]');
    search.addEventListener('input', () => {
      const query = search.value.trim().toLocaleLowerCase();
      let shown = 0;
      cards.forEach((card) => {
        const match = card.dataset.search.includes(query);
        card.hidden = !match;
        if (match) shown += 1;
      });
      if (empty) empty.hidden = shown !== 0;
    });
  }

  const setupForm = document.querySelector('[data-setup-form]');
  if (setupForm) {
    const submitButton = setupForm.querySelector('[data-setup-submit]');
    const providerButton = setupForm.querySelector('[data-provider-verify]');
    const providerStatus = setupForm.querySelector('[data-provider-status]');
    const unifiButton = setupForm.querySelector('[data-unifi-verify]');
    const unifiStatus = setupForm.querySelector('[data-unifi-status]');
    const unifiSite = setupForm.querySelector('[data-unifi-site]');
    const providerFields = [
      'passcode',
      'google_auth_client_id',
      'google_oauth_client_secret'
    ].map((name) => setupForm.elements.namedItem(name));
    const unifiFields = [
      'passcode',
      'unifi_network_api_url',
      'unifi_api_key',
      'trust_unifi_self_signed_certificate'
    ].map((name) => setupForm.elements.namedItem(name));
    let providersVerified = false;
    let unifiVerified = false;

    const updateSubmit = () => {
      submitButton.disabled = !providersVerified || !unifiVerified;
    };
    const resetProviders = () => {
      providersVerified = false;
      providerStatus.className = 'verify-status';
      providerStatus.textContent = 'Credentials not tested.';
      updateSubmit();
    };
    const resetUnifi = () => {
      unifiVerified = false;
      unifiSite.disabled = true;
      unifiSite.replaceChildren(new Option('Test connection to load sites', ''));
      unifiStatus.className = 'verify-status';
      unifiStatus.textContent = 'Connection not tested.';
      updateSubmit();
    };
    providerFields.forEach((field) => field.addEventListener('input', resetProviders));
    providerFields.forEach((field) => field.addEventListener('change', resetProviders));
    unifiFields.forEach((field) => field.addEventListener('input', resetUnifi));
    unifiFields.forEach((field) => field.addEventListener('change', resetUnifi));
    unifiSite.addEventListener('change', () => {
      unifiVerified = Boolean(unifiSite.value);
      updateSubmit();
    });

    const verify = async (button, status, fields, url, pending, failed, success) => {
      const missing = fields.filter((field) => field.type !== 'checkbox')
        .find((field) => !field.value.trim());
      if (missing) {
        missing.reportValidity();
        missing.focus();
        return false;
      }
      button.disabled = true;
      status.className = 'verify-status pending';
      status.textContent = pending;
      const body = new URLSearchParams();
      fields.forEach((field) => {
        if (field.type !== 'checkbox' || field.checked) body.set(field.name, field.value);
      });
      try {
        const response = await fetch(url, {
          method: 'POST',
          headers: {'Content-Type': 'application/x-www-form-urlencoded'},
          body
        });
        const message = await response.text();
        if (!response.ok) throw new Error(message || failed);
        status.className = 'verify-status success';
        status.textContent = message;
        success();
        updateSubmit();
        return true;
      } catch (error) {
        status.className = 'verify-status error';
        status.textContent = error.message;
        return false;
      } finally {
        button.disabled = false;
      }
    };

    providerButton.addEventListener('click', () => {
      providersVerified = false;
      updateSubmit();
      verify(
        providerButton,
        providerStatus,
        providerFields,
        '/setup/verify-providers',
        'Testing Google OAuth credentials…',
        'Google OAuth verification failed',
        () => { providersVerified = true; }
      );
    });
    unifiButton.addEventListener('click', async () => {
      unifiVerified = false;
      updateSubmit();
      const missing = unifiFields.filter((field) => field.type !== 'checkbox')
        .find((field) => !field.value.trim());
      if (missing) {
        missing.reportValidity();
        missing.focus();
        return;
      }
      unifiSite.disabled = true;
      unifiSite.replaceChildren(new Option('Testing connection…', ''));
      unifiButton.disabled = true;
      unifiStatus.className = 'verify-status pending';
      unifiStatus.textContent = 'Testing controller connection…';
      const body = new URLSearchParams();
      unifiFields.forEach((field) => {
        if (field.type !== 'checkbox' || field.checked) body.set(field.name, field.value);
      });
      try {
        const response = await fetch('/setup/verify-unifi', {
          method: 'POST',
          headers: {'Content-Type': 'application/x-www-form-urlencoded'},
          body
        });
        if (!response.ok) throw new Error(await response.text() || 'UniFi verification failed');
        const result = await response.json();
        unifiSite.replaceChildren(new Option('Choose a site', ''));
        result.sites.forEach((site) => unifiSite.add(new Option(`${site.name} — ${site.id}`, site.id)));
        unifiSite.disabled = false;
        unifiStatus.className = 'verify-status success';
        unifiStatus.textContent = result.message;
        unifiSite.focus();
      } catch (error) {
        resetUnifi();
        unifiStatus.className = 'verify-status error';
        unifiStatus.textContent = error.message;
      } finally {
        unifiButton.disabled = false;
      }
    });

  }
  document.querySelectorAll('time[data-unix]').forEach((el) => {
    const value = Number(el.dataset.unix) * 1000;
    if (Number.isFinite(value)) el.textContent = new Date(value).toLocaleString();
  });
})();
