const urlParams = new URLSearchParams(window.location.hash.substring(1));
let chart = null;

async function run() {
  const container = document.getElementById('charts');
  const element = document.createElement('div');
  container.appendChild(element);

  const response = await fetch('./overall.json');
  const data = await response.json();

  let visible = [
    data.series[0].name,
    data.series[1].name,
    data.series[2].name,
    data.series[3].name,
  ];
  if (urlParams.get('series'))
    visible = urlParams.get('series').split(',');
  const visibleSet = {};
  for (let series of visible)
    visibleSet[series] = true;

  let max = 0;
  const series = data.series.map((s, i) => {
    s.data = s.data.map((point, i) => {
      if (point > max)
        max = point;
      return [Date.parse(data.commits[i].date), point];
    });
    s.visible = s.name in visibleSet;

    return s;
  });

  chart = Highcharts.chart(element, {
    chart: {
      height: '100%',
    },
    title: {
      text: 'TITLE',
    },
    yAxis: {
      labels: {
        formatter: function() { return format(this.value); },
      },
      title: {
        text: 'Time (seconds)',
      },
    },
    xAxis: {
      type: 'datetime',
      dateTimeLabelFormats: {
        month: '%e. %b',
        year: '%b',
      },
      title: {
        text: 'Commit index',
      },
    },
    tooltip: {
      useHTML: true,
      style: {
        pointerEvents: 'auto',
      },
      headerFormat: '<b>{series.name}</b><br>',
      pointFormatter: function() {
        let text = '<small>length:</small> ' + format(this.y);
        const commit = data.commits[this.index].sha;
        text += '<br><small>sha:</small> ' + commit.substring(0, 8);
        const microarch = data.commits[this.index].jobs[this.series.name].cpu_microarch;
        if (microarch !== null)
          text += '<br><small>cpu:</small> ' + microarch;
        return text;
      },
    },
    series,
    plotOptions: {
      series: {
        events: {
          show: updateHash,
          hide: updateHash,
        },
        point: {
          events: {
            click: event => {
              const commit = data.commits[event.point.index].sha;
              const name = event.point.series.name;
              window.location = `commit.html#job=${name}&commit=${commit}`;
            }
          },
        },
      },
    },
  });

  document.getElementById('loading').remove();
}

document.addEventListener('DOMContentLoaded', run);

function get_mut(map, k, default_) {
  if (map[k] === undefined) {
    map[k] = default_;
  }
  return map[k];
}


function format(s) {
  if (s < 60) {
    return `${Math.floor(s)}s`;
  }
  s /= 60;
  if (s < 60) {
    return `${Math.floor(s * 100) / 100}m`;
  }
  return `${Math.floor(s / 60)}h${Math.floor(s % 60)}m`;
}

function updateHash() {
  const series = [];
  for (let row of chart.series)
    if (row.visible)
      series.push(row.name);
  window.location.hash = '#series=' + series.join(',');
}
