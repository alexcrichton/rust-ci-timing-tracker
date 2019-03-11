const table = document.createElement('table');
const urlParams = new URLSearchParams(window.location.search);
const job = urlParams.get('job');
let sortCol = 0;
let sortDesc = true;

const response = fetch('overall.json');
let allCommits = response.then(r => r.json());
let showRelative = false;

const commits = {};

async function run() {
  const relative = document.getElementById('relative');
  showRelative = relative.checked;
  relative.onclick = () => {
    showRelative = relative.checked;
    renderTable();
  };

  const paramRelative = urlParams.get('relative');
  if (paramRelative === 'true') {
    showRelative = true;
    relative.checked = true;
  } else if (paramRelative === 'false') {
    showRelative = false;
    relative.checked = false;
  }

  const paramCol = urlParams.get('col');
  if (paramCol !== null)
    sortCol = parseInt(paramCol);
  const paramDesc = urlParams.get('desc');
  if (paramDesc === 'true')
    sortDesc = true;
  else if (paramDesc === 'false')
    sortDesc = false;

  const promises = [];
  for (let commit of urlParams.get('commit').split(','))
    promises.push(loadCommit(commit));
  await Promise.all(promises);
  await renderTable();
  document.getElementById('loading').remove();
  document.body.appendChild(table);
  document.getElementById('controls').style.display = 'block';

  const loadCommitText = document.getElementById('load-commit');
  document.querySelector('#load-commit-link').onclick = async () => {
    const commit = loadCommitText.value;
    loadCommitText.value = '';
    await loadCommit(commit);
    await renderTable();
  };

}

async function loadCommit(commit) {
  try {
    console.log('loading', commit);
    const response = await fetch(commit + '.json');
    const json = await response.json();
    json.sha = commit;
    commits[commit] = json;
  } catch (e) {
    alert('commit ' + commit + ' does not exist');
    throw e;
  }
}

async function renderTable() {
  while (table.firstChild)
    table.removeChild(table.firstChild);
  const titles = document.createElement('tr');
  table.appendChild(titles);

  const order = await commitOrder();
  const steps = [];
  const stepSet = {};
  const first = {};

  const stepTitle = document.createElement('td');
  stepTitle.textContent = `${job} steps`;
  titles.appendChild(stepTitle);

  for (let item of order) {
    if (item.commit === undefined)
      continue;
    for (let step in item.commit.jobs[job].timings) {
      if (step in stepSet)
        continue;

      const add = step => {
        const row = document.createElement('tr');
        table.appendChild(row);
        const name = document.createElement('td');
        name.textContent = prettyStep(step);
        name.dataset.name = step;
        name.classList.add('name');
        row.appendChild(name);
        stepSet[step] = row;
        steps.push(step);
      };
      add(step);
      for (let part in item.commit.jobs[job].timings[step].parts) {
        add(`${part} - ${step}`);
      }
    }
  }

  for (let item of order) {
    if (item.next !== undefined) {
      titles.appendChild(loadNextTd(item.next));
    } else if (item.prev !== undefined) {
      titles.appendChild(loadPrevTd(item.prev));
    } else {
      const timings = item.commit.jobs[job].timings;

      for (let step of steps) {
        const add = (step, dur) => {
          const row = stepSet[step];
          const value = document.createElement('td');

          if ((step in first) && showRelative) {
            const rel = dur - first[step];
            value.dataset.value = rel;
            const relText = Math.round(rel * 100) / 100;
            let html = '';
            if (rel > 0) {
              html = `<span class='slower'>+${relText}s</span>`;
            } else {
              html = `<span class='faster'>${relText}s</span>`;
            }
            html += ` (${prettyDur(dur)})`;
            value.innerHTML = html;
          } else {
            first[step] = dur;
            value.dataset.value = dur;
            value.textContent = prettyDur(dur);
          }
          row.appendChild(value);
        };
        const s = step.split(' - ');
        if (s.length == 1) {
          const dur = step in timings ? timings[step].dur : 0;
          add(step, dur);
        } else {
          let dur = 0;
          if (s[1] in timings) {
            if (s[0] in timings[s[1]].parts) {
              dur = timings[s[1]].parts[s[0]];
            }
          }
          add(step, dur);
        }
      }

      const title = document.createElement('td');
      title.innerHTML = `
        <a href='${item.commit.jobs[job].url}'>${item.commit.sha.substring(0, 8)}</a>
        ·
        <a href='#' class='sort'</a>
      `;
      title.dataset.commit = item.commit.sha;
      titles.appendChild(title);
    }
  }

  const rows = table.querySelectorAll('tr').length;
  for (let td of table.querySelectorAll('td.load')) {
    td.rowSpan = rows;
  }

  sortTable();
}

function updateUrl() {
  const shas = [];
  for (let td of table.querySelectorAll('tr:first-child td')) {
    if (td.dataset.commit)
      shas.push(td.dataset.commit);
  }

  let query = `?job=${job}`;
  query += `&commit=${shas.join(',')}`;
  query += `&relative=${showRelative}`;
  query += `&col=${sortCol}`;
  query += `&desc=${sortDesc}`;

  if (window.location.search != query) {
    window.history.pushState({}, "Title", "commit.html" + query);
  }

}

async function commitOrder() {
  const keys = Object.keys(commits);
  if (keys.length == 1) {
    return [
      { 'prev': keys[0] },
      { 'commit': commits[keys[0]] },
      { 'next': keys[0] },
    ];
  }
  const all = await allCommits;
  const ret = [];
  for (let i = 0; i < all.commits.length; i++) {
    const sha = all.commits[i].sha;
    if (sha in commits) {
      if (i > 0 && !(all.commits[i - 1].sha in commits))
        ret.push({ 'prev': sha });
      ret.push({ 'commit': commits[sha] });
      if (i + 1 < all.commits.length && !(all.commits[i + 1].sha in commits))
        ret.push({ 'next': sha });
    }
  }
  return ret;
}

function prettyStep(step) {
  step = step.replace(', compare_mode: None', '');
  step = step.replace(/unknown-linux-gnu/g, 'linux');
  step = step.replace(/pc-windows-gnu/g, 'mingw');
  step = step.replace(/pc-windows-msvc/g, 'msvc');
  step = step.replace(/, mode: Rustc/g, '');
  step = step.replace(/, mode: Std/g, '');
  step = step.replace(/, mode: Test/g, '');
  step = step.replace(/, test_kind: Test/g, ', test');
  step = step.replace(/, is_optional_tool: false/g, '');
  step = step.replace(/, source_type: InTree/g, '');
  step = step.replace(/, extra_features: \[\]/g, '');
  step = step.replace(/compiler: Compiler/g, 'Compiler');
  step = step.replace(/, tool: ".*?"/g, '');
  step = step.replace(/, mode: Tool\w+/g, '');
  step = step.replace(/, version: MdBook./g, '');
  step = step.replace(/, path: Some\(".*?"\)/g, '');
  step = step.replace(/, mode: ".*?"/g, '');
  step = step.replace(/Compiler \{ stage: (.*?), host: "(.*?)" \}/g, (_, m1, m2) => {
    return `compiler: "${m1}/${m2}"`
  });

  return step;
}

function prettyDur(s) {
  if (s < 60) {
    return `${Math.floor(s)}s`;
  }
  s /= 60;
  if (s < 60) {
    return `${Math.floor(s * 100) / 100}m`;
  }
  return `${Math.floor(s / 60)}h${Math.floor(s % 60)}m`;
}

function sortTable() {
  const sorts = table.querySelectorAll('tr:first-child td a.sort');
  sorts.forEach((a, i) => {
    a.textContent = "⇳";
    delete a.dataset.descending;
    a.onclick = () => {
      sortCol = i;
      sortDesc = true;
      if (sorts[sortCol].dataset.descending === 'true')
        sortDesc = false;
      sortTable();
      return false;
    };
  });

  sorts[sortCol].dataset.descending = sortDesc;
  if (sortDesc)
    sorts[sortCol].textContent = "⬇";
  else
    sorts[sortCol].textContent = "⬆";

  const rows = [];
  for (let row of table.querySelectorAll('tr:not(:first-child)')) {
    row.remove();
    rows.push(row);
  }

  const m = sortDesc ? -1 : 1;
  rows.sort((a, b) => {
    let aval = parseFloat(a.querySelector(`td:nth-child(${sortCol + 2})`).dataset.value);
    let bval = parseFloat(b.querySelector(`td:nth-child(${sortCol + 2})`).dataset.value);
    if (aval < bval)
      return -1 * m;
    else if (aval > bval)
      return 1 * m;
    return 0;
  });

  for (let row of rows)
    table.appendChild(row);

  updateUrl();
}

async function loadPreviousCommit(event) {
  const all = await allCommits;
  const current = event.target.dataset.commit;

  for (let i = 0; i < all.commits.length; i++) {
    if (all.commits[i].sha !== current || i == 0)
      continue;
    await loadCommit(all.commits[i - 1].sha);
    break;
  }
  await renderTable();
  return false;
}

async function loadNextCommit() {
  const all = await allCommits;
  const current = event.target.dataset.commit;

  for (let i = 0; i < all.commits.length; i++) {
    if (all.commits[i].sha !== current || i === all.commits.length - 1)
      continue;
    await loadCommit(all.commits[i + 1].sha);
    break;
  }
  await renderTable();
  return false;
}

function loadPrevTd(commit) {
  const td = document.createElement('td');
  const a = document.createElement('a');
  a.href = '#';
  a.dataset.commit = commit;
  a.onclick = loadPreviousCommit;
  a.textContent = '⭅';
  td.appendChild(a);
  td.classList.add("load");
  return td;
}

function loadNextTd(commit) {
  const td = document.createElement('td');
  const a = document.createElement('a');
  a.href = '#';
  a.dataset.commit = commit;
  a.onclick = loadNextCommit;
  a.textContent = '⭆';
  td.appendChild(a);
  td.classList.add("load");
  return td;
}

run();
